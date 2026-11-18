use std::{io, path::PathBuf};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {cause}", crate::remote::display(path))]
    Io { path: PathBuf, cause: io::Error },

    #[error("{}: no index found (looked for {tried}); {how}", crate::remote::display(path))]
    MissingIndex { path: PathBuf, tried: String, how: String },

    #[error("invalid locus `{text}`: {cause}")]
    InvalidLocus { text: String, cause: ggv_model::LocusParseError },

    /// `file` names the file when the caller can't tell which of several it was (the
    /// reference, say, rather than the alignments the locus was typed for).
    #[error("contig `{name}` is not in {} (it has e.g. {})", file.as_deref().unwrap_or("this file"), examples.join(", "))]
    UnknownContig { name: String, examples: Vec<String>, file: Option<String> },

    #[error("{}: not a BAM file{hint}: {cause}", crate::remote::display(path))]
    NotAlignments { path: PathBuf, hint: &'static str, cause: io::Error },

    #[error("{locus} starts past the end of {contig} (length {length})")]
    OutOfBounds { locus: String, contig: String, length: u64 },
}

impl Error {
    /// Names the reference as the file an unknown contig is missing from.
    #[must_use]
    pub fn in_reference(self, path: &std::path::Path) -> Self {
        match self {
            Self::UnknownContig { name, examples, file: None } => {
                let file = Some(format!("the reference {}", crate::remote::file_name(path)));
                Self::UnknownContig { name, examples, file }
            }
            other => other,
        }
    }
}

pub(crate) trait IoContext<T> {
    fn at(self, path: &std::path::Path) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn at(self, path: &std::path::Path) -> Result<T> {
        self.map_err(|cause| Error::Io { path: path.to_owned(), cause })
    }
}
