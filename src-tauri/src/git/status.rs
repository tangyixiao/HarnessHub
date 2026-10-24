use super::{GitCaptureError, StatusCounts};

pub(super) fn parse_status(output: &[u8]) -> Result<StatusCounts, GitCaptureError> {
    let mut counts = StatusCounts::default();
    let mut cursor = 0;

    while cursor < output.len() {
        let record_end = output[cursor..]
            .iter()
            .position(|byte| *byte == 0)
            .map(|offset| cursor + offset)
            .ok_or(GitCaptureError::MalformedOutput)?;
        let record = &output[cursor..record_end];
        if record.len() < 4 || record[2] != b' ' || record[3..].is_empty() {
            return Err(GitCaptureError::MalformedOutput);
        }

        let x = record[0];
        let y = record[1];
        let conflict = matches!(
            (x, y),
            (b'D', b'D')
                | (b'A', b'U')
                | (b'U', b'D')
                | (b'U', b'A')
                | (b'D', b'U')
                | (b'A', b'A')
                | (b'U', b'U')
        );
        let untracked = (x, y) == (b'?', b'?');
        if !valid_status_byte(x)
            || !valid_status_byte(y)
            || (x == b'?' || y == b'?') && !untracked
            || (x == b'U' || y == b'U') && !conflict
            || (x == b' ' && y == b' ')
        {
            return Err(GitCaptureError::MalformedOutput);
        }

        if x == b'R' || x == b'C' || y == b'R' || y == b'C' {
            cursor = record_end + 1;
            let source_end = output[cursor..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|offset| cursor + offset)
                .ok_or(GitCaptureError::MalformedOutput)?;
            if source_end == cursor {
                return Err(GitCaptureError::MalformedOutput);
            }
            cursor = source_end + 1;
        } else {
            cursor = record_end + 1;
        }

        if conflict {
            increment(&mut counts.conflicted_files)?;
        } else if untracked {
            increment(&mut counts.untracked_files)?;
        } else {
            if x != b' ' {
                increment(&mut counts.staged_files)?;
            }
            if y != b' ' {
                increment(&mut counts.unstaged_files)?;
            }
        }
    }

    Ok(counts)
}

fn valid_status_byte(byte: u8) -> bool {
    matches!(
        byte,
        b' ' | b'M' | b'A' | b'D' | b'R' | b'C' | b'U' | b'T' | b'?'
    )
}

fn increment(count: &mut u64) -> Result<(), GitCaptureError> {
    *count = count
        .checked_add(1)
        .ok_or(GitCaptureError::MalformedOutput)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_status;

    #[test]
    fn parses_empty_status() {
        assert_eq!(parse_status(b"").unwrap(), Default::default());
    }

    #[test]
    fn counts_staged_and_unstaged_for_same_file() {
        let counts = parse_status(b"MM file\0").unwrap();
        assert_eq!(counts.staged_files, 1);
        assert_eq!(counts.unstaged_files, 1);
    }

    #[test]
    fn counts_each_untracked_path() {
        let counts = parse_status(b"?? dir/a\0?? dir/b\0").unwrap();
        assert_eq!(counts.untracked_files, 2);
    }

    #[test]
    fn rename_and_copy_continuations_count_once() {
        assert_eq!(parse_status(b"R  new\0old\0").unwrap().staged_files, 1);
        assert_eq!(parse_status(b"C  new\0old\0").unwrap().staged_files, 1);
    }

    #[test]
    fn seven_conflict_codes_only_increment_conflicted() {
        let counts = parse_status(b"DD a\0AU b\0UD c\0UA d\0DU e\0AA f\0UU g\0").unwrap();
        assert_eq!(counts.conflicted_files, 7);
        assert_eq!(counts.staged_files, 0);
        assert_eq!(counts.unstaged_files, 0);
    }

    #[test]
    fn accepts_non_utf8_and_control_bytes_in_path() {
        let counts = parse_status(b"?? line\nbreak\ttab\xff\0").unwrap();
        assert_eq!(counts.untracked_files, 1);
    }

    #[test]
    fn rejects_truncation_and_unknown_xy_without_path_leaks() {
        for output in [
            b"M".as_slice(),
            b"M ",
            b"?x path\0",
            b"Z  secret-name\0",
            b"R  new\0",
        ] {
            let error = parse_status(output).unwrap_err();
            assert_eq!(error.to_string(), "malformed Git status output");
            assert!(!error.to_string().contains("secret-name"));
        }
    }

    #[test]
    fn rejects_unmerged_marker_outside_supported_conflict_pairs() {
        for output in [b"U  file\0".as_slice(), b"MU file\0", b"UM file\0"] {
            assert_eq!(
                parse_status(output).unwrap_err(),
                super::GitCaptureError::MalformedOutput
            );
        }
    }
}
