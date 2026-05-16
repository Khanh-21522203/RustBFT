#[derive(thiserror::Error, Debug)]
pub enum CodecError {
    #[error("unexpected eof")]
    Eof,
    #[error("invalid data: {0}")]
    Invalid(&'static str),
}
