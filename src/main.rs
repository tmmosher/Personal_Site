// TODO break out functions into modules
mod server {
    use anyhow::{anyhow, Error};
    use axum::http::header::{CONTENT_TYPE, LOCATION};
    use axum::response::Response;
    use axum::{body::Body, extract::{rejection::JsonRejection, ConnectInfo, State}, http::{HeaderMap, HeaderValue, StatusCode}, response::{IntoResponse, Redirect}, routing::get, Json, Router};
    use chrono::Utc;
    use lazy_static::lazy_static;
    use regex::Regex;
    use serde::{Deserializer, Serialize};
    use serde_json::{to_value, Value};
    use sqlx::{sqlite, sqlite::{SqliteConnectOptions, SqliteJournalMode}, Executor, Pool};
    use std::{
        env,
        net::SocketAddr,
        sync::Arc,
    };
    use tera::Tera;

    // Page templating
    lazy_static! {
        pub static ref TEMPLATES: Tera = {
            let source = "src/templates/**/*.html";
            match Tera::new(source) {
                Ok(t) => {
                    println!("Source template compiled correctly");
                    t
                },
                Err(e) => {
                    println!("Parsing error(s) encountered: {}", e);
                    ::std::process::exit(1);
                }
            }
        };
    }

    // constants
    const ROOT: &str = "http://localhost:3000/";

    //Role map:
    // 2: User
    // 1: Mod
    // 0: Admin
    // role map is not used in database as sqlite doesn't like enums.
    // May refactor for User display function later
    enum Role {
        USER,
        MOD,
        ADMIN
    }

    #[derive(Serialize, Debug, sqlx::FromRow)]
    struct User {
        // size of values will not change while in-memory, ergo String type safely replaced by Box<str>
        username: Box<str>,
        last_online: Box<str>,
        created: Box<str>,
        role: u32
    }

    impl User {
        fn new(username: Box<str>, role: u32) -> Self {
            User {
                username,
                last_online: Box::from(Utc::now().to_rfc3339()),
                created: Box::from(Utc::now().to_rfc3339()),
                role
            }
        }

        fn set_role(&mut self, role: u32) {
            self.role = role;
        }
    }

    pub struct AppState {
        read_pool: Pool<sqlite::Sqlite>,
        write_pool: Pool<sqlite::Sqlite>,
        per_page: u32
    }

    #[tokio::main(flavor = "multi_thread")]
    pub(crate) async fn main() {
        let shared_state = bootstrap().await;
        let app = Router::new()
            .route("/", get(root))
            .route("/users", get(users_list_route))
            .route("/user/{}", get(get_user_route))
            .route("/api/users", get(get_users).post(post_user))
            .fallback(unknown_path)
            .with_state(shared_state);
        // obviously if these fail the issue is irrecoverable, therefore 'expect' is reasonable to use.
        let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.expect("Bind failed");
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.expect("Serving failed");
    }

    /// Creates or connects to database needed for internal application state.
    // as this is a function run at startup, this uses unsafe functions like expect() and can fail.
    async fn bootstrap() -> Arc<AppState> {
        let database = match dotenvy::dotenv() {
            Ok(_buf) => {
                println!("Loaded env variables!");
                env::var("DATABASE_URL").expect("DATABASE_URL environment variable not found.")
            }
            Err(e) => {
                eprintln!("Failed to parse env variables: {}", e);
                ::std::process::exit(1);
            }
        };
        println!("Database URL: {}", database);
        let read_conn_opt: SqliteConnectOptions = SqliteConnectOptions::new()
            .filename(&database)
            .journal_mode(SqliteJournalMode::Wal)
            .read_only(true)
            .create_if_missing(true);
        let write_conn_opt: SqliteConnectOptions = SqliteConnectOptions::new()
            .filename(&database)
            .journal_mode(SqliteJournalMode::Wal)
            .create_if_missing(true);
        println!("Acquired read connection.");
        let read_conn: sqlite::SqlitePool = sqlite::SqlitePool::connect_lazy_with(read_conn_opt);
        let write_conn: sqlite::SqlitePool = sqlite::SqlitePool::connect_lazy_with(write_conn_opt);
        let query = "
    CREATE TABLE IF NOT EXISTS user_table (id INTEGER PRIMARY KEY, username TEXT NOT NULL, last_online TEXT NOT NULL, created TEXT NOT NULL, role INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS post_table (id INTEGER PRIMARY KEY, title TEXT NOT NULL, post TEXT NOT NULL);
    ";
        write_conn.acquire().await.expect("Failed to acquire write connection in 'bootstrap()'")
            .execute(query).await.expect("Failed to create user and post table in 'bootstrap()'");
        Arc::new(AppState { read_pool: read_conn, write_pool: write_conn, per_page: 32 })
    }

    /// Home page
    async fn root() -> Response {
        let mut context = tera::Context::new();
        context.insert("ROOT", ROOT);
        let page = TEMPLATES.render("index.html", &context);
        match page {
            // return a tuple parsable to an axum::Response
            Ok(page) => {
                (
                    StatusCode::OK,
                    [("Content-Type", "text/html")],
                    Body::from(page)
                ).into_response()
            }
            Err(_e) => {
                println!("Failed to create page: {:?}", _e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [("Content-Type", "text/html")],
                    Body::from("<h1>Internal server error. Please contact site administrator for help.<h1>")
                ).into_response()
            }
        }
    }

    async fn users_list_route(State(state): State<Arc<AppState>>) -> Response {
        let mut context = tera::Context::new();
        context.insert("page_no", &1);
        context.insert("ROOT", ROOT);
        if let Ok(users) = get_users_by_pagination(state).await {
            context.insert("users", &users);
        } else {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("Content-Type", "text/html")],
                Body::from("<h1>Internal server error: Cannot display users.<h1>")
            ).into_response()
        }
        //TODO pagination
        let page = TEMPLATES.render("users.html", &context);
        match page {
            //return a tuple parsable to an axum::response to satisfy return impl
            Ok(page) => {
                (
                    StatusCode::OK,
                    [("Content-Type", "text/html")],
                    Body::from(page)
                ).into_response()
            }
            Err(_e) => {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [("Content-Type", "text/html")],
                    Body::from("<h1>Internal server error: Cannot display page.<h1>")
                ).into_response()
            }
        }
    }

    // TODO implementation
    async fn get_user_route(State(state): State<Arc<AppState>>) -> Response {
        (
            StatusCode::OK,
            [("Content-Type", "text/html")],
            Body::from("Hello! Under construction..")
        ).into_response()
    }

    ///    API endpoint to return users as a JSON list.
    async fn get_users(State(state): State<Arc<AppState>>) -> Response {
        let body = match get_users_by_pagination(state).await {
            Ok(t) => to_value(t),
            Err(_e) => to_value(format!("{}", _e))
        };
        match body {
            Ok(body) => {
                (
                    StatusCode::OK,
                    [("Content-Type", "application/json")],
                    Body::from(body.to_string())
                ).into_response()
            }
            Err(_) => {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [("Content-Type", "text/plain")],
                    Body::from("Internal server error")
                ).into_response()
            }
        }
    }

    /// Handles detailed account creation and database access. Returns either a valid/invalid
    /// response ready to be sent back to client or a server error to fn 'post_user'.
    async fn post_user_body(state: State<Arc<AppState>>, add_user_status: Result<User, (StatusCode, String)>)
                            -> Result<impl IntoResponse, anyhow::Error> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_str("text/plain")?);
        match add_user_status {
            // 'add_user_status' match block determines if we are going
            // to add a new user OR return to fn 'post_user' based on if 'add_user_status'
            // indicates the user data is structurally valid.
            Ok(user) => match select_by_username(&user.username, &state).await {
                // inner match block to determine if database has Some User associated with the
                // given username.
                None => {
                    // user is not a duplicate, can be created
                    insert_user(&user, &state).await?;
                    headers.insert(LOCATION, HeaderValue::from_str(format!("{ROOT}/user/{}", user.username).as_str())?);
                    Ok((
                        StatusCode::CREATED,
                        headers,
                        Body::default()
                    ))
                },
                Some(matching_user_or_error) => {
                    // either the database found a matching user or returned an error
                    match matching_user_or_error {
                        Ok(_v) => {
                            Ok((
                                StatusCode::BAD_REQUEST,
                                headers,
                                Body::from("User already exists.")
                            ))
                        },
                        Err(_e) => Err(anyhow!("Unable to determine user status."))
                    }
                }
            },
            //Despite being an Err case, this is a valid response to bubble up to fn 'post_user' for
            // it to build as a non-server error response.
            Err((code, reason)) => {
                Ok((
                    code,
                    headers,
                    Body::from(reason)
                ))
            }
        }
    }

    /// POST request handler for account creation.
    async fn post_user(state: State<Arc<AppState>>, result: Result<Json<Value>, JsonRejection>)
                       -> Response {
        // extracts user information from the POST body
        let user_status = match result {
            Ok(Json(json_map)) => {
                let res = json_map.get("username");
                // make sure content is valid
                username_check(res)
            },
            // more specific JSON error handling for response as per the axum::extract docs
            Err(err) => match err {
                JsonRejection::JsonSyntaxError(_) => Err((StatusCode::BAD_REQUEST, "Invalid JSON syntax.".to_string())),
                JsonRejection::JsonDataError(_) => Err((StatusCode::BAD_REQUEST, "Given JSON data structure does not match expected parsed result.".to_string())),
                JsonRejection::MissingJsonContentType(_) =>  Err((StatusCode::BAD_REQUEST, "Missing JSON content type in request header.".to_string())),
                JsonRejection::BytesRejection(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to buffer request body.".to_string())),
                _ => Err((StatusCode::INTERNAL_SERVER_ERROR, "Unknown error".to_string())),
            }
        };
        post_user_body(state, user_status).await.map_or_else(|_e| {
            // error condition, could provide more details but I would need to sanitize first.
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [("Content-Type", "text/plain")],
                    Body::from("Internal server error. Contact site administrator for assistance.")
                ).into_response()
            }, |v| v.into_response())
    }

    /// Validates username contains no special characters (underscores permitted) and is at least 5 letters/numbers long.
    /// Must include at least one letter.
    fn username_check(json_value: Option<&Value>) -> Result<User, (StatusCode, String)> {
        // if the extractor passes and a username field exists + is valid, evaluates to a new user.
        // For obvious security reasons only users (role lvl 2) can be created via the API.
        json_value.and_then(|username_json| username_json.as_str())
            .and_then(|name| {
                // rust's regex engine doesn't support look-aheads for some reason, so this checks
                // for at least 5 alphanumeric values, with at least one of them being strictly alphabetic
                if Regex::new(r"^[_a-zA-Z0-9]{5,32}$").is_ok_and(|val| val.is_match(name))
                    && name.chars().any(|c| c.is_alphabetic()){
                     Some(User::new(Box::from(name), 2))
                } else {
                    None
                }
            })
            .ok_or((StatusCode::BAD_REQUEST, "JSON payload structure invalid.".to_string()))
    }

    /// Find a given user in the database by username
    async fn select_by_username(username: &str, state: &State<Arc<AppState>>) -> Option<Result<User, anyhow::Error>> {
        let read_conn = &state.read_pool;
        sqlx::query_as("SELECT * FROM user_table WHERE username = $1 LIMIT 1")
            .bind(username)
            .fetch_optional(read_conn)
            .await
            // branch depending on error status of query. If db has an issue, we have SOME ERRor to
            // return. If we have SOME OK value, we return that too. If method 'and_then' fails in the
            // success branch of 'map_or_else', we implicitly return None. This is a bit clearer
            // than the nested matches in my opinion and allows for a switch to an Optional Result.
            .map_or_else(|error| Some(Err(anyhow!("Internal server error: {error}."))),
                            |row| row.map(Ok))
    }

    /// Inserts a user into persistent storage.
    async fn insert_user(user: &User, state: &State<Arc<AppState>>) -> Result<bool, anyhow::Error> {
        let write_conn = &state.write_pool;
        let insert_statement = sqlx::query("INSERT INTO user_table (username, last_online, created, role)
        VALUES ($1, $2, $3, $4)")
            .bind(&*user.username.to_string())
            .bind(user.last_online.to_string())
            .bind(user.created.to_string())
            .bind(user.role)
            .execute(write_conn).await?;
        match insert_statement.rows_affected() {
            1 => Ok(true),
            _ => Err(anyhow!("Unable to create user.")),
        }
    }

    //TODO implement 'pagination' part of 'get_users_by_pagination'
    /// Retrieves a vector of User structs comprised of the first n=state.per_page users.
    async fn get_users_by_pagination(state: Arc<AppState>) -> Result<Vec<User>, sqlx::error::Error> {
        sqlx::query_as("SELECT * FROM user_table ORDER BY username LIMIT $1")
            .bind(state.per_page)
            .fetch_all(&state.read_pool)
            .await
    }

    async fn unknown_path() -> Redirect {
        Redirect::to("/")
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use assertables::{assert_err, assert_ok};
        #[test]
        fn test_valid_user_api_post_value() {
            let json = to_value("Water_Bottle".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_ok!(result);
            let json = to_value("Water_Bottle123".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_ok!(result);
            let json = to_value("123Water_Bottle".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_ok!(result);
            let json = to_value("1234f".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_ok!(result);
        }
        #[test]
        fn test_invalid_user_api_post_type() {
            let json = to_value(true).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
            let json = to_value(1).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
            let json = to_value([1, 5]).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
            let json = to_value(["test", "test_string_vec"]).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
        }

        #[test]
        fn test_invalid_user_api_post_name() {
            let json = to_value("  f".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
            let json = to_value("f  ".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
            let json = to_value("   ".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
            let json = to_value("DELETE * FROM user_table WHERE 1=1;".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
            let json = to_value("1234".to_string()).unwrap();
            let result = username_check(Some(&json));
            assert_err!(result);
        }
        
    }
}
fn main() {
    server::main();
}