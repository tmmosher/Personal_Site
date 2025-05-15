use axum::{routing::{get, post}, Router, extract::{Path, ConnectInfo, State}, response::{IntoResponse, Html, Redirect}};
use std::{
    sync::Arc,
    net::SocketAddr
};
use tokio::sync::RwLock;

struct AppState {
    foo_response: RwLock<String>
}

#[tokio::main]
async fn main() {
    let shared_state = Arc::new(AppState {foo_response: RwLock::new(String::from("Foo"))});
    let app = Router::new()
        .route("/", get(root))
        .route("/foo", get(get_foo))
        .route("/change_foo/{new_foo}", post(post_foo))
        .route("/foo/bar", get(foo_bar_stranger))
        .route("/foo/bar/{user_name}", get(foo_bar))
        .fallback(unknown_path)
        .with_state(shared_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
}

async fn root(ConnectInfo(addr): ConnectInfo<SocketAddr>) -> Html<String> {
    format!("<h1>Hello, World!</h1><h2> You are connecting from: {}</h2>", addr).into()
}

async fn get_foo(State(state): State<Arc<AppState>>) -> Html<String> {
    let foo_status = state.foo_response.read().await;
    format!("<h1>Can I get a foo? {}</h1>", *foo_status).into()
}

async fn post_foo(State(state): State<Arc<AppState>>, Path(new_foo): Path<String>) -> Html<String>{
    let mut foo_status = state.foo_response.write().await;
    println!("Old foo: {}", *foo_status);
    *foo_status = new_foo.clone();
    format!("<h1>New foo is {}!</h1>",  *foo_status).into()
}

async fn foo_bar_stranger() -> Html<String> {
    Html(String::from("Hello, stranger!"))
}

async fn foo_bar(path : Option<Path<String>>) -> impl IntoResponse {
    let user_name= path.unwrap();
    format!("Hello, {}!", user_name.0)
}

async fn unknown_path() -> Redirect {
    Redirect::to("/")
}