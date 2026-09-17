#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TagError {
    #[error("release tag is empty")]
    Empty,
    #[error("release tag is a reserved directory name")]
    Reserved,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Io(String),
    #[error("the version currently in use cannot be removed")]
    RemoveActive,
    #[error("this operation requires multiple versions to be enabled")]
    RequiresMultiVersion,
    #[error("no version is currently active")]
    NoActiveVersion,
    #[error("no installed version named {0}")]
    NoSuchVersion(String),
    #[error(transparent)]
    Tag(#[from] TagError),
}

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("network request failed: {0}")]
    Http(String),
    #[error(
        "GitHub's rate limit is reached. Unauthenticated requests are limited to 60 per hour; try again later."
    )]
    RateLimited,
    #[error("could not understand GitHub's response: {0}")]
    Json(String),
    #[error("{0}")]
    Io(String),
    #[error(transparent)]
    Tag(#[from] TagError),
    #[error("cancelled")]
    Cancelled,
    #[error("unrecognised archive format")]
    UnknownArchiveFormat,
    #[error("could not extract the downloaded archive: {0}")]
    ExtractionFailed(String),
    #[error("{0}")]
    LaunchFailed(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}
