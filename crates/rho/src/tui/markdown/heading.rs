#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum HeadingLevel {
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AtxHeading<'a> {
    pub(super) level: HeadingLevel,
    pub(super) content: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HeadingStreamState {
    Potential,
    Heading,
    NotHeading,
}

pub(super) fn parse_atx_heading(line: &str) -> Option<AtxHeading<'_>> {
    let (hashes, after_hashes) = opening_hashes(line)?;
    if let Some(first) = after_hashes.chars().next() {
        if !matches!(first, ' ' | '\t') {
            return None;
        }
    }

    let body = after_hashes.trim_start_matches([' ', '\t']);
    Some(AtxHeading {
        level: heading_level(hashes),
        content: trim_closing_hashes(body),
    })
}

pub(super) fn heading_stream_state(line: &str) -> HeadingStreamState {
    let Some((hashes, after_hashes)) = opening_hashes(line) else {
        return if line.len() <= 3 && line.bytes().all(|byte| byte == b' ') {
            HeadingStreamState::Potential
        } else {
            HeadingStreamState::NotHeading
        };
    };
    debug_assert!((1..=6).contains(&hashes));

    match after_hashes.chars().next() {
        None => HeadingStreamState::Potential,
        Some(' ' | '\t') => HeadingStreamState::Heading,
        Some(_) => HeadingStreamState::NotHeading,
    }
}

fn opening_hashes(line: &str) -> Option<(usize, &str)> {
    let indent = line.bytes().take_while(|byte| *byte == b' ').count();
    if indent > 3 {
        return None;
    }

    let rest = &line[indent..];
    let hashes = rest.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    Some((hashes, &rest[hashes..]))
}

fn heading_level(hashes: usize) -> HeadingLevel {
    match hashes {
        1 => HeadingLevel::H1,
        2 => HeadingLevel::H2,
        3 => HeadingLevel::H3,
        4 => HeadingLevel::H4,
        5 => HeadingLevel::H5,
        6 => HeadingLevel::H6,
        _ => unreachable!("opening_hashes only returns one through six hashes"),
    }
}

fn trim_closing_hashes(body: &str) -> &str {
    let body = body.trim_end_matches([' ', '\t']);
    let closing_start = body.trim_end_matches('#').len();
    if closing_start == body.len() {
        return body;
    }
    if closing_start == 0
        || body[..closing_start]
            .chars()
            .next_back()
            .is_some_and(|ch| matches!(ch, ' ' | '\t'))
    {
        body[..closing_start].trim_end_matches([' ', '\t'])
    } else {
        body
    }
}

#[cfg(test)]
#[path = "heading_tests.rs"]
mod tests;
