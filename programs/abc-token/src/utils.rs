use std::error::Error;
use std::fmt;

#[derive(Debug)]
pub struct UtilError {
    message: String,
}

impl UtilError {
    fn new(msg: &str) -> UtilError {
        UtilError {
            message: msg.to_string(),
        }
    }
}

impl fmt::Display for UtilError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for UtilError {}

/// Trims whitespace and ensures string is not empty
pub fn validate_string(input: &str) -> Result<String, UtilError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Err(UtilError::new("String cannot be empty"))
    } else {
        Ok(trimmed.to_string())
    }
}

/// Validates number is within specified range
pub fn validate_range(num: i32, min: i32, max: i32) -> Result<i32, UtilError> {
    if num < min || num > max {
        Err(UtilError::new(&format!(
            "Number must be between {} and {}", 
            min, max
        )))
    } else {
        Ok(num)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_string() {
        assert!(validate_string("  test  ").is_ok());
        assert!(validate_string("").is_err());
        assert!(validate_string("    ").is_err());
    }

    #[test]
    fn test_validate_range() {
        assert!(validate_range(5, 0, 10).is_ok());
        assert!(validate_range(-1, 0, 10).is_err());
        assert!(validate_range(11, 0, 10).is_err());
    }
}