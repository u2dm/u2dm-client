use crate::domain::models::LoginCredentials;

pub enum UiCommand {
    CheckServer(String),
    LoginPassword(LoginCredentials),
}
