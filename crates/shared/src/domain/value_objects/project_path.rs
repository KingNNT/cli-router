use crate::domain::DomainError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectPath(String);

impl ProjectPath {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let s = value.into();
        if s.is_empty() {
            Err(DomainError::InvalidProjectPath)
        } else {
            Ok(Self(s))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_accepts_non_empty() {
        let p = ProjectPath::new("/home/me/work").unwrap();
        assert_eq!(p.as_str(), "/home/me/work");
    }

    #[test]
    fn new_rejects_empty() {
        assert_eq!(ProjectPath::new(""), Err(DomainError::InvalidProjectPath));
    }
}
