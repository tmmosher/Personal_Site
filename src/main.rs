// TODO break out functions into modules
mod server {
    use anyhow::anyhow;
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
        // size of values will not change while in-memory, no need for String
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
                role }
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
            .route("/users", get(users_route))
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

    async fn users_route(State(state): State<Arc<AppState>>) -> Response {
        let mut context = tera::Context::new();
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
        context.insert("page_no", &1);
        context.insert("ROOT", ROOT);
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
                println!("Failed to render page: {:?}", _e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [("Content-Type", "text/html")],
                    Body::from("<h1>Internal server error: Cannot display page.<h1>")
                ).into_response()
            }
        }
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

    //TODO implement 'pagination' part of 'get_users_by_pagination'
    /// Retrieves a vector of User structs comprised of the first n=state.per_page users.
    async fn get_users_by_pagination(state: Arc<AppState>) -> Result<Vec<User>, sqlx::error::Error> {
        sqlx::query_as("SELECT * FROM user_table ORDER BY username LIMIT $1")
            .bind(state.per_page)
            .fetch_all(&state.read_pool)
            .await
    }

    /// Handles detailed account creation and database access. Returns either a response ready to be sent
    /// or an error to fn 'post_user'.
    async fn post_user_body(state: State<Arc<AppState>>, result: Result<User, (StatusCode, String)>)
                            -> Result<impl IntoResponse, anyhow::Error> {
        //HeaderMap is more readable here, as responses get ugly when using a list of tuples
        // for multiple headers.
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_str("text/plain")?);
        let new_user = match result {
            Ok(user) => user,
            //Despite being an Err case, this is a valid response to bubble up to fn post_user
            Err((code, reason)) => {
                return Ok((
                    code,
                    headers,
                    Body::from(reason)
                ))
            }
        };
        //TODO this inverted logic here is pretty bad. Once Result -> Option refactor is complete for
        // select_by_username this will be refactored to be much clearer
        match select_by_username(&new_user.username, &state).await {
            // if select_by_username found something, then it's a duplicate name and must be rejected
            Ok(_) => {
                Ok((
                    StatusCode::BAD_REQUEST,
                    headers,
                    Body::from("User already exists.")
                ))
            },
            // 
            Err(anyhow) => {
                match anyhow.to_string().as_str() {
                    "No users found with this username." => {
                        insert_user(&new_user, &state).await?;
                        headers.insert(LOCATION, HeaderValue::from_str(format!("{ROOT}/user/{}", new_user.username).as_str())?);
                        Ok((
                            StatusCode::CREATED,
                            headers,
                            Body::default()
                        ))
                    },
                    _ => {
                        Err(anyhow!("Internal server error"))
                    }
                }
            }
        }
    }

    /// POST request handler for account creation.
    async fn post_user(state: State<Arc<AppState>>, result: Result<Json<Value>, JsonRejection>)
                       -> Response {
        // unwraps user information from the POST body
        let user_status = match result {
            Ok(Json(json_map)) => {
                let res = json_map.get("username");
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
        match post_user_body(state, user_status).await {
            // matches all expected behavior including good and bad responses (bad request, bytes rejection,
            // improper JSON, etc.)
            Ok(v) => {
                v.into_response()
            },
            // matches all uncaught behavior. This should not be encountered often (ideally ever).
            Err(_) => {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [("Content-Type", "text/plain")],
                    Body::from("Internal server error. Contact site administrator for assistance.")
                ).into_response()
            }
        }
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

    // TODO refactor target from result to option
    async fn select_by_username(username: &str, state: &State<Arc<AppState>>) -> Result<User, anyhow::Error> {
        let read_conn = &state.read_pool;
        let p_stmnt = sqlx::query_as("SELECT * FROM user_table WHERE username = $1 LIMIT 1")
            .bind(username)
            .fetch_optional(read_conn)
            .await;
        // TODO turn this into a functional unwrap rather than match statement
        match p_stmnt {
            Ok(v) => {
                match v {
                    Some(v) => Ok(v),
                    None => Err(anyhow!("No users found with this username."))
                }
            },
            Err(_) => {
                Err(anyhow!("Internal server error."))
            }
        }
    }

    /// Inserts a user from memory into persistent storage.
    /* Result is used rather than Option for unpacking with the '?'
        operator to help code readability during the query. Furthermore, Axum **really** doesn't like
        returning Options, but it seems to be happy with Results.*/
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