use std::fmt;

#[derive(Debug)]
pub enum CliError {
    User(String),
    System(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::User(m) | CliError::System(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CliError {}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::User(_) => 1,
            CliError::System(_) => 2,
        }
    }
}

pub fn user<S: Into<String>>(msg: S) -> CliError {
    CliError::User(msg.into())
}

pub fn system<S: Into<String>>(msg: S) -> CliError {
    CliError::System(msg.into())
}

pub type CliResult<T> = Result<T, CliError>;
