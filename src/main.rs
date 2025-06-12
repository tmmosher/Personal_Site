use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::{body::Body, extract::{rejection::JsonRejection, ConnectInfo, State}, http::{HeaderMap, HeaderValue, StatusCode}, response::{IntoResponse, Redirect}, routing::get, Json, Router};
use chrono::prelude::*;
use lazy_static::lazy_static;
use serde::{Serialize, Deserialize};
use serde_json::{to_value, Value};
use sqlx::{sqlite, sqlite::{SqliteConnectOptions, SqliteJournalMode}, Executor, Pool};
use std::{
    net::SocketAddr,
    sync::Arc,
};
use anyhow::{anyhow, Error};
use axum::http::HeaderName;
use axum::response::Response;
use tera::Tera;

#[derive(Deserialize)]
struct Config {
    database_url: String,
}

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

lazy_static! {
    pub static ref database: String = {
        match envy::from_env::<Config>() {
            Ok(conf) => {
                println!("Loaded env variables");
                conf.database_url
            }
            Err(e) => {
                eprintln!("Failed to parse env variables: {}", e);
                ::std::process::exit(1);
            }
        }
    };
}

//Role map:
// 2: User
// 1: Mod
// 0: Admin
#[derive(Serialize, Debug, sqlx::FromRow)]
struct User {
    username: String,
    last_online: DateTime<Utc>,
    created: DateTime<Utc>,
    role: u32
}

impl User {
    fn new(username: String, role: u32) -> Self {
        User { username, last_online: Utc::now(), created: Utc::now(), role }
    }

    fn set_role(&mut self, role: u32) {
        self.role = role;
    }
}

struct AppState {
    read_pool: Pool<sqlite::Sqlite>,
    write_pool: Pool<sqlite::Sqlite>,
    per_page: u32
}

#[tokio::main]
async fn main() {
    let shared_state = bootstrap().await;
    let app = Router::new()
        .route("/", get(root))
        .route("/users", get(users))
        .route("/api/users", get(get_users).post(post_user))
        .fallback(unknown_path)
        .with_state(shared_state);
    // obviously if these fail the issue is irrecoverable, therefore 'expect' is reasonable to use.
    let listener = tokio::net::TcpListener::bind("localhost:3000").await.expect("Bind failed");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.expect("Serving failed");
}

async fn bootstrap() -> Arc<AppState>{
    let read_conn_opt: SqliteConnectOptions = SqliteConnectOptions::new()
        .filename(database.as_str())
        .journal_mode(SqliteJournalMode::Wal)
        .read_only(true)
        .create_if_missing(true);
    let write_conn_opt: SqliteConnectOptions = SqliteConnectOptions::new()
        .filename(database.as_str())
        .journal_mode(SqliteJournalMode::Wal)
        .create_if_missing(true);
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
async fn root(ConnectInfo(addr): ConnectInfo<SocketAddr>) -> impl IntoResponse {
    let mut context = tera::Context::new();
    context.insert("adr", &addr.to_string());
    let page = TEMPLATES.render("index.html", &context);
    match page {
        // return a tuple parsable to an axum::response
        Ok(page) => {
            (
                StatusCode::OK,
                [("Content-Type", "text/html")],
                Body::from(page)
            )
        }
        Err(_e) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("Content-Type", "text/html")],
                Body::from("<h1>Internal server error. Please contact site administrator for help.<h1>")
            )
        }
    };
}

async fn users(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut context = tera::Context::new();
    //TODO Implement DB reading for users
    // users = //here!;
    //TODO pagination
    context.insert("page_no", &1);
    let page = TEMPLATES.render("users.html", &context);
    match page {
        //return a tuple parsable to an axum::response to satisfy return impl
        Ok(page) => {
            (
                StatusCode::OK,
                [("Content-Type", "text/html")],
                Body::from(page)
            )
        }
        Err(_e) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("Content-Type", "text/html")],
                Body::from("<h1>Internal server error: Cannot display users.<h1>")
            )
        }
    }
}

/*
    API endpoint to return users as a JSON list.
 */
async fn get_users(state: State<Arc<AppState>>) -> impl IntoResponse {
    //TODO implement api 'users' endpoint for GET with db integration
    let body = match get_users_by_pagination(state).await {
        Ok(t) => to_value(t),
        Err(e) => to_value("")
    };
    match body {
        Ok(body) => {
            (
                StatusCode::OK,
                [("Content-Type", "application/json")],
                Body::from(body.to_string())
            )
        }
        Err(_) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("Content-Type", "text/plain")],
                Body::from("Internal server error")
            )
        }
    }
    
}

async fn get_users_by_pagination (state: State<Arc<AppState>>) -> Result<Vec<User>, sqlx::error::Error> {
    sqlx::query_as("SELECT * FROM users ORDER BY last_online ASC LIMIT $1")
        .bind(state.per_page)
        .fetch_all(&state.read_pool)
        .await
}

async fn post_user_body(state: State<Arc<AppState>>, result: Result<User, (StatusCode, String)>) 
    -> Result<impl IntoResponse, anyhow::Error> {
    //unwraps the user
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_str("text/plain")?);
    let new_user = match result {
        Ok(user) => user,
        // Despite being an Err case, this is a valid response to return to the user.
        Err((code, reason)) => {
            return Ok((
                    code,
                    headers,
                    Body::from(reason)
                ))
        }
    };
    match select_by_username(&new_user.username, &state).await {
        Ok(_) => {
            Ok((
                StatusCode::BAD_REQUEST,
                headers,
                Body::from("User already exists.")
            ))
        },
        Err(anyhow) => {
            match anyhow.to_string().as_str() {
                "No users found with this username." => {
                    insert_user(&new_user, &state).await?;
                    //TODO change from localhost:3000 when moving away from local testing
                    //headers.insert("Location", HeaderValue::from_str(format!("https://tmmosher.com/user/{}", new_user.username).as_str())?);
                    headers.insert("Location", HeaderValue::from_str(format!("https://localhost:3000/user/{}", new_user.username).as_str())?);
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

async fn post_user(state: State<Arc<AppState>>, result: Result<Json<Value>, JsonRejection>)
    -> Response {
    let user_status = match result {
        Ok(Json(value)) => match value.get("username") {
            // if the extractor passes and a username field exists, evaluates to a new user.
            // do note, dear reader, that this doesn't do any pattern checking for a username.
            // I should probably add size limits later, but for now this will suffice.
            // For obvious security reasons only users (role lvl 2) can be created via the API.
            Some(name) => Ok(User::new(name.to_string(), 2)),
            // JSON format is valid but doesn't contain the required field 'username'
            None => Err((StatusCode::BAD_REQUEST, "JSON payload improperly structured".to_string())),
        },
        // more specific JSON error handling for response as per the axum::extract docs
        Err(err) => match err {
            JsonRejection::JsonSyntaxError(_) => Err((StatusCode::BAD_REQUEST, "JSON payload improperly structured".to_string())),
            JsonRejection::BytesRejection(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to buffer request body".to_string())),
            _ => Err((StatusCode::INTERNAL_SERVER_ERROR, "Unknown error".to_string())),
        }
    };
    match post_user_body(state, user_status).await {
        // matches all defined behavior, including 'bad' responses (BAD_REQUEST, bytes rejection, 
        // improper JSON, etc.)
        Ok(v) => {
            v.into_response()
        },
        // matches all undefined behavior, essentially just server errors. 
        Err(_) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("Content-Type", "text/plain")],
                Body::from("Internal server error")
            ).into_response()
        }
    }
}

async fn select_by_username(username: &str,  state: &State<Arc<AppState>>) -> Result<User, anyhow::Error> {
    let read_conn = &state.read_pool;
    let p_stmnt = sqlx::query_as("SELECT * FROM users WHERE username = $1 LIMIT 1")
        .bind(username.to_string())
        .fetch_optional(read_conn)
        .await;
    // manually unwrap p_stmnt for better error handling response
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

async fn insert_user(user: &User, state: &State<Arc<AppState>>) -> Result<bool, anyhow::Error> {
    let write_conn = &state.write_pool;
    let insert_statement = sqlx::query("INSERT INTO users (username, last_online, created, role)
        VALUES ($1, $2, $3, $4)")
        .bind(user.username.clone())
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