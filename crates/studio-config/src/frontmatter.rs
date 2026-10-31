use thiserror::Error;

#[derive(Error, Debug, PartialEq, Eq)]
pub enum FrontmatterError {
    #[error("frontmatter end marker missing")]
    MissingEnd,
}

fn line_starts_with_marker(text: &str, offset: usize) -> bool {
    text[offset..].starts_with("---")
        && text
            .as_bytes()
            .get(offset + 3)
            .is_none_or(|b| *b == b'\n' || *b == b'\r')
}

pub fn split(text: &str) -> Result<Option<(&str, &str)>, FrontmatterError> {
    if !line_starts_with_marker(text, 0) {
        return Ok(None);
    }
    let mut offset = 3;
    while let Some(&b) = text.as_bytes().get(offset) {
        if b == b'\n' {
            offset += 1;
            break;
        }
        offset += 1;
    }
    let yaml_start = offset;
    let Some(end) = text[offset..].find("\n---") else {
        return Err(FrontmatterError::MissingEnd);
    };
    let yaml_end = offset + end + 1;
    let body_start = yaml_end + 4;
    let yaml = text[yaml_start..yaml_end].trim();
    let body = &text[body_start..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    Ok(Some((yaml, body)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_and_body() {
        let text = "---\nid: test\n---\nbody here\n";
        let (yaml, body) = split(text).unwrap().unwrap();
        assert_eq!(yaml, "id: test");
        assert_eq!(body, "body here\n");
    }

    #[test]
    fn no_frontmatter_returns_none() {
        assert_eq!(split("no frontmatter").unwrap(), None);
    }

    #[test]
    fn four_dashes_is_not_frontmatter() {
        assert_eq!(split("----\nnot frontmatter\n").unwrap(), None);
    }

    #[test]
    fn missing_end_is_error() {
        assert_eq!(
            split("---\nid: test\n").unwrap_err(),
            FrontmatterError::MissingEnd
        );
    }
}
