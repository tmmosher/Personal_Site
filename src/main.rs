use axum::{body::Body, extract::{ConnectInfo, State, Path, rejection::JsonRejection}, http::{HeaderMap, HeaderValue, StatusCode}, response::{IntoResponse, Redirect, Response}, routing::get, Json, Router};
use lazy_static::lazy_static;
use serde::Serialize;
use sqlx::{Pool, sqlite, sqlite::{SqliteConnectOptions, SqliteJournalMode}, Executor};
use serde_json::{json, to_value, Value};
use std::{
    net::SocketAddr,
    sync::Arc,
    collections::HashMap,
};
use std::error::Error;
use axum::http::header::{CONTENT_TYPE, LOCATION};
use tera::Tera;
use chrono::prelude::*;

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

#[derive(Serialize, Debug, sqlx::FromRow)]
struct User {
    username: String,
    last_online: DateTime<Utc>,
    created: DateTime<Utc>,
    role: Role
}

impl User {
    fn new(username: String, role: Role) -> Self {
        User { username, last_online: Utc::now(), created: Utc::now(), role }
    }

    fn set_role(&mut self, role: Role) {
        self.role = role;
    }
}

#[derive(Serialize, Debug)]
enum Role {
    ADMIN,
    USER,
    MOD
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
        .filename("uap.db")
        .journal_mode(SqliteJournalMode::Wal)
        .read_only(true)
        .create_if_missing(true);
    let write_conn_opt: SqliteConnectOptions = SqliteConnectOptions::new()
        .filename("uap.db")
        .journal_mode(SqliteJournalMode::Wal)
        .create_if_missing(true);
    let read_conn: sqlite::SqlitePool = sqlite::SqlitePool::connect_lazy_with(read_conn_opt);
    let write_conn: sqlite::SqlitePool = sqlite::SqlitePool::connect_lazy_with(write_conn_opt);
    let query = "
    CREATE TABLE IF NOT EXISTS user_table (id INTEGER PRIMARY KEY, username TEXT NOT NULL);
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

//TODO rip out old user system

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
async fn get_users(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    //TODO implement api 'users' endpoint for GET with db integration
    let body = match to_value(&*users) {
        Ok(t) => t.to_string(),
        Err(_e) => "".to_string()
    };
    (
        StatusCode::OK,
        [("Content-Type", "application/json")],
        Body::from(body)
        )
}

async fn post_user(state: State<Arc<AppState>>, result: Result<Json<Value>, JsonRejection>)
    -> Result<impl IntoResponse, anyhow::Error> {
    //TODO a lot of explicit matching here. Definitely improved with use of 'anyhow' but could probably
    // stand to be improved further.
    let user_status = match result {
        Ok(Json(value)) => match value.get("username") {
            // if the extractor passes and a username field exists, evaluates to a new user.
            // do note, dear reader, that this doesn't do any pattern checking for a username.
            // I should probably add size limits later, but for now this will suffice.
            // For obvious security reasons only users can be created via the API.
            Some(name) => Ok(User::new(name.to_string(), Role::USER)),
            None => Err((StatusCode::BAD_REQUEST, "JSON payload improperly structured".to_string())),
        },
        // more specific JSON error handling for response as per the axum::extract docs
        Err(err) => match err {
            JsonRejection::JsonSyntaxError(_) => Err((StatusCode::BAD_REQUEST, "JSON payload improperly structured".to_string())),
            JsonRejection::BytesRejection(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to buffer request body".to_string())),
            _ => Err((StatusCode::INTERNAL_SERVER_ERROR, "Unknown error".to_string())),
        }
    };
    //unwraps the user
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_str("text/plain")?);
    let new_user = match user_status {
        Ok(user) => user,
        // Despite being an Err case, this is a valid response to return to the user.
        Err((code, reason)) => {
            return
                Ok((
                    code,
                    headers,
                    Body::from(reason)
                ))
        }
    };
    // TODO add DB user select
    let response = match users.get(&new_user.username) {
        Some(_) => {
            (
                StatusCode::BAD_REQUEST,
                headers,
                Body::from("User already exists.")
            )
        },
        None => {
            //TODO add DB user addition
            users.insert(new_user.username.clone(), new_user);
            let location = format!("https://tmmosher.com/user/{}", new_user.username).as_str();
            headers.insert(LOCATION, HeaderValue::from_str(location)?);
            (
                StatusCode::CREATED,
                headers,
                Body::default()
            )
        }
    };
    Ok(response)
}

async fn select_by_username(username: &str,  state: &State<Arc<AppState>>) -> Result<User, anyhow::Error> {
    let read_conn = state.read_pool.acquire().await?;
    let p_stmnt = sqlx::query("SELECT 1 FROM users WHERE username = $1;");
    //TODO temp for compiler to shut up
    Ok(User::new(username.to_string(), Role::USER))
}

async fn unknown_path() -> Redirect {
    Redirect::to("/")
}