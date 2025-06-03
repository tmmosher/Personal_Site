use axum::{body::Body, extract::{ConnectInfo, State, Path, rejection::JsonRejection}, http::StatusCode, response::{IntoResponse, Redirect, Response}, routing::get, Json, Router};
use lazy_static::lazy_static;
use serde::Serialize;
use serde_json::{json, to_value, Value};
use std::{
    net::SocketAddr,
    sync::Arc,
    collections::HashMap,
};
use tera::Tera;
use tokio::sync::RwLock;

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

#[derive(Serialize)]
struct User {
    //id: Uuid,
    username: String,
}

struct Pagination {
    page: u32,
    per_page: u32,
}

// this abstraction for in-memory users is in the process of being replaced by a DB approach
impl User {
    // fn new(username: String) -> Self {
    //     User { id: Uuid::new_v4(), username }
    // }
    
    //just for text output of a user
    fn format(&self, to_add: &User, list: &mut Vec<String>) {
        list.push(format!("   - Username: {}", to_add.username))
    }
}

struct AppState {
    user_map: RwLock<HashMap<String, User>>,
    
}

#[tokio::main]
async fn main() {
    let shared_state = bootstrap();
    let app = Router::new()
        .route("/", get(root))
        .route("/users", get(users))
        .route("/api/users", get(get_users).post(post_user))
        .fallback(unknown_path)
        .with_state(shared_state);
    let listener = tokio::net::TcpListener::bind("localhost:3000").await.unwrap();
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
}

fn bootstrap() -> Arc<AppState>{
    //TODO implement sqlx connection pooling with separate reader / writer connections. 
    // let connection = //here;
    let query = "
    CREATE TABLE IF NOT EXISTS user_table (id INTEGER PRIMARY KEY, username TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS post_table (id INTEGER PRIMARY KEY, title TEXT NOT NULL, post TEXT NOT NULL);
    ";
    //TODO Placeholder to make compiler happy
    Arc::new(AppState { user_map: RwLock::new(HashMap::new())})
}
async fn root(ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Response {
    let mut context = tera::Context::new();
    context.insert("adr", &addr.to_string());
    let page = TEMPLATES.render("index.html", &context);
    match page {
        Ok(page) => {
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html")
                .body(Body::from(page))
                .unwrap()
        }
        Err(_e) => {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("Content-Type", "text/html")
                .body(Body::from("<h1>Page not found.<h1>"))
                .unwrap()
        }
    }

}

async fn users(State(state): State<Arc<AppState>>) -> Response {
    let mut context = tera::Context::new();
    let users = state.user_map.read().await;
    // turn user_map into an iterator of values and collect cloned username and ID
    // into a vector for rendering.
    //TODO pagination
    context.insert("users", &users.values().map(|u| u.username.clone()).collect::<Vec<_>>());
    let page = TEMPLATES.render("users.html", &context);
    match page {
        Ok(page) => {
            Response::builder()
                .header("Content-Type", "text/html")
                .status(StatusCode::OK)
                .body(Body::from(page))
                .unwrap()
        }
        Err(_e) => {
            Response::builder()
                .header("Content-Type", "text/html")
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::from("<h1>Internal server error: Cannot display users.<h1>"))
                .unwrap()
        }
    }
}

/*
    API endpoint to return users as a JSON list.
 */
async fn get_users(State(state): State<Arc<AppState>>) -> Response {
    let users = state.user_map.read().await;
    let body = match to_value(&*users) {
        Ok(t) => t.to_string(),
        Err(_e) => to_value("").unwrap().to_string()
    };
    Response::builder()
        .header("Content-Type", "application/json")
        .status(StatusCode::OK)
        .body(Body::from(body))
        .unwrap()
}

async fn post_user(state: State<Arc<AppState>>, result: Result<Json<Value>, JsonRejection>) -> Response {
    let user_status = match result {
        Ok(Json(value)) => match value.get("username") {
            // if the extractor passes and a username field exists, evaluates to a new user.
            // do note, dear reader, that this doesn't do any pattern checking for a username.
            // I should probably add size limits later, but for now this will suffice.
            Some(name) => Ok(User { username: name.to_string() }),
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
    let new_user = match user_status {
        Ok(user) => user,
        Err((code, reason)) => {
            return Response::builder()
                .header("Content-Type", "text/plain")
                .status(code)
                .body(Body::from(reason))
                .unwrap()
        }
    };
    let mut users = state.user_map.write().await;
    let response = match users.get(&new_user.username) {
        Some(_) => {
            Response::builder()
                .header("Content-Type", "text/plain")
                .status(StatusCode::BAD_REQUEST)
                .body(Body::from("User already exists."))
                .unwrap()
        },
        None => {
            //TODO make the string a literal here? 'new_user.username' gets inserted surrounded by quotations..
            let link =  format!("http://localhost:3000/api/user/{}", new_user.username);
            //TODO 'insert' could fail - add inner match case
            users.insert(new_user.username.clone(), new_user);
            Response::builder()
                .header("Content-Type", "text/plain")
                .header("Location", link)
                .status(StatusCode::CREATED)
                .body(Body::default())
                .unwrap()
        }
    };
    response
}

async fn unknown_path() -> Redirect {
    Redirect::to("/")
}

// async fn get_foo(State(state): State<Arc<AppState>>) -> Html<String> {
//     let foo_status = state.foo_response.read().await;
//     format!("<h1>Do you know what a foo is? {}</h1>", *foo_status).into()
// }
//
// async fn post_foo(State(state): State<Arc<AppState>>, Path(new_foo): Path<String>) -> Html<String>{
//     let mut foo_status = state.foo_response.write().await;
//     *foo_status = new_foo.clone();
//     format!("<h1>New foo is {}!</h1>",  *foo_status).into()
// }
//
// async fn foo_bar_stranger() -> Html<String> {
//     Html(String::from("Hello, stranger!"))
// }
//
// async fn foo_bar(path : Option<Path<String>>) -> impl IntoResponse {
//     let user_name= path.unwrap();
//     format!("Hello, {}!", user_name.0)
// }
