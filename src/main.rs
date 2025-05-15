use axum::{routing::{get, post}, Router, extract::{Path, ConnectInfo, State}, response::{IntoResponse, Html, Redirect}};
use std::{
    sync::Arc,
    net::SocketAddr
};
use tokio::sync::RwLock;

struct User {
    id: i32,
    username: String,
}

impl User {
    fn new(id: i32, username: String) -> Self {
        User { id, username }
    }
    
    //during implementation, may have to change the user_list to mutable 
    fn format(&self, to_add: &User, list: &mut Vec<String>) {
        list.push(format!("   - ID: {} | Username: {}", to_add.id, to_add.username))
    }
}

struct AppState {
    user_list: RwLock<Vec<User>>
}

#[tokio::main]
async fn main() {
    let shared_state = Arc::new(AppState { user_list: RwLock::new(Vec::with_capacity(10))});
    let app = Router::new()
        .route("/", get(root))
        .route("/users", get(users))
        // .route("/users/add", post())
        .fallback(unknown_path)
        .with_state(shared_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
}

async fn root(ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Html<String> {
    format!("<h1>Hello, World!</h1><h2> You are connecting from: {}</h2>", addr).into()
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

async fn users(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    
}

async fn unknown_path() -> Redirect {
    Redirect::to("/")
}