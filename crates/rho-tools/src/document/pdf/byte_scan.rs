//! Byte-level PDF nesting and stream-budget preflight before any lopdf load.

use std::collections::BTreeMap;
use std::io::Read as _;

use flate2::read::ZlibDecoder;

pub(in crate::document) const MAX_PDF_EXPANDED_STREAM_BYTES: usize = 64 * 1024 * 1024;
/// Caps nested PDF arrays/dicts before lopdf 0.41 parses untrusted bytes.
///
/// lopdf 0.41 (pulled by `pdf-inspector`) bounds nested literal strings but not
/// array/dictionary depth. This byte scan runs before any parser load.
pub(in crate::document) const MAX_PDF_OBJECT_NESTING_DEPTH: usize = 100;

pub(in crate::document) fn validate_object_nesting(bytes: &[u8]) -> Result<(), String> {
    let mut index = 0;
    let mut depth = 0;
    let mut remaining = MAX_PDF_EXPANDED_STREAM_BYTES;
    let integer_objects = collect_direct_integer_objects(bytes);

    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                index += 1;
                while index < bytes.len() && !matches!(bytes[index], b'\r' | b'\n') {
                    index += 1;
                }
            }
            b'(' => {
                let mut string_depth = 1;
                index += 1;
                while index < bytes.len() && string_depth > 0 {
                    match bytes[index] {
                        b'\\' => index = (index + 2).min(bytes.len()),
                        b'(' => {
                            string_depth += 1;
                            index += 1;
                        }
                        b')' => {
                            string_depth -= 1;
                            index += 1;
                        }
                        _ => index += 1,
                    }
                }
            }
            b'<' if bytes.get(index + 1) == Some(&b'<') => {
                depth += 1;
                check_object_nesting_depth(depth)?;
                index += 2;
            }
            b'>' if bytes.get(index + 1) == Some(&b'>') => {
                depth = depth.saturating_sub(1);
                index += 2;
            }
            b'<' => {
                index += 1;
                while index < bytes.len() && bytes[index] != b'>' {
                    index += 1;
                }
                index += usize::from(index < bytes.len());
            }
            b'[' => {
                depth += 1;
                check_object_nesting_depth(depth)?;
                index += 1;
            }
            b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b's' if stream_keyword_at(bytes, index) => {
                index = skip_stream_and_budget(bytes, index, &integer_objects, &mut remaining)?;
            }
            _ => index += 1,
        }
    }
    Ok(())
}

fn check_object_nesting_depth(depth: usize) -> Result<(), String> {
    if depth > MAX_PDF_OBJECT_NESTING_DEPTH {
        return Err(format!(
            "PDF object nesting depth {depth} exceeds the {MAX_PDF_OBJECT_NESTING_DEPTH} level limit"
        ));
    }
    Ok(())
}

fn stream_keyword_at(bytes: &[u8], index: usize) -> bool {
    let keyword = b"stream";
    let Some(end) = index.checked_add(keyword.len()) else {
        return false;
    };
    bytes.get(index..end) == Some(keyword)
        && index > 0
        && bytes[index - 1].is_ascii_whitespace()
        && bytes.get(end).is_some_and(u8::is_ascii_whitespace)
}

fn skip_stream_and_budget(
    bytes: &[u8],
    stream_at: usize,
    integer_objects: &BTreeMap<u32, i64>,
    remaining: &mut usize,
) -> Result<usize, String> {
    let keyword_end = stream_at + b"stream".len();
    let data_start = stream_data_start(bytes, keyword_end)
        .ok_or_else(|| "PDF stream is missing a line break after the stream keyword".to_owned())?;
    let dict = dict_before_stream(bytes, stream_at);
    let length = dict.and_then(|dict| stream_length(dict, integer_objects));
    let (content, after_content) = match length {
        Some(length) => {
            let data_end = data_start
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| "PDF stream length exceeds the remaining file bytes".to_owned())?;
            let after = skip_stream_end_marker(bytes, data_end)?;
            (&bytes[data_start..data_end], after)
        }
        None => {
            // Fall back when /Length is absent or not resolvable. Prefer /Length
            // so embedded "endstream" bytes inside the payload are not cut short.
            let Some(end_offset) = bytes[data_start..]
                .windows(b"endstream".len())
                .position(|window| window == b"endstream")
            else {
                return Err("PDF stream is missing an endstream marker".to_owned());
            };
            let data_end = data_start + end_offset;
            let after = data_end + b"endstream".len();
            (&bytes[data_start..data_end], after)
        }
    };

    if let Some(dict) = dict {
        budget_stream_bytes(dict, content, remaining)?;
    } else {
        // No dict means we cannot classify filters. Count raw size only.
        consume_budget(remaining, content.len())?;
    }

    Ok(after_content)
}

fn stream_data_start(bytes: &[u8], keyword_end: usize) -> Option<usize> {
    match bytes.get(keyword_end)? {
        b'\n' => Some(keyword_end + 1),
        b'\r' if bytes.get(keyword_end + 1) == Some(&b'\n') => Some(keyword_end + 2),
        b'\r' => Some(keyword_end + 1),
        _ => None,
    }
}

fn skip_stream_end_marker(bytes: &[u8], mut index: usize) -> Result<usize, String> {
    if bytes.get(index) == Some(&b'\r') {
        index += 1;
    }
    if bytes.get(index) == Some(&b'\n') {
        index += 1;
    }
    let end = index
        .checked_add(b"endstream".len())
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| "PDF stream is missing an endstream marker".to_owned())?;
    if &bytes[index..end] != b"endstream" {
        return Err("PDF stream end does not match the declared /Length".to_owned());
    }
    Ok(end)
}

fn dict_before_stream(bytes: &[u8], stream_at: usize) -> Option<&[u8]> {
    let mut end = stream_at;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end < 2 || &bytes[end - 2..end] != b">>" {
        return None;
    }

    let mut depth = 1usize;
    let mut index = end - 2;
    while index > 0 {
        index -= 1;
        match (bytes[index], bytes.get(index + 1).copied()) {
            (b'<', Some(b'<')) => {
                depth -= 1;
                if depth == 0 {
                    return Some(&bytes[index..end]);
                }
            }
            (b'>', Some(b'>')) => depth += 1,
            _ => {}
        }
    }
    None
}

fn stream_length(dict: &[u8], integer_objects: &BTreeMap<u32, i64>) -> Option<usize> {
    let value = trim_pdf_whitespace(dict_value_after_name(dict, b"/Length")?);
    let (first, rest) = parse_pdf_u32_prefix(value)?;
    let rest = trim_pdf_whitespace(rest);
    // Prefer `n gen R` so "/Length 12 0 R" is not read as bare length 12.
    if let Some((generation, after_gen)) = parse_pdf_u32_prefix(rest) {
        let after_gen = trim_pdf_whitespace(after_gen);
        if after_gen.starts_with(b"R")
            && after_gen
                .get(1)
                .is_none_or(|byte| is_pdf_name_delimiter(*byte))
        {
            if generation != 0 {
                return None;
            }
            return integer_objects
                .get(&first)
                .copied()
                .and_then(|number| usize::try_from(number).ok());
        }
    }
    if rest.is_empty() || is_pdf_name_delimiter(rest[0]) {
        return usize::try_from(first).ok();
    }
    None
}

fn budget_stream_bytes(dict: &[u8], content: &[u8], remaining: &mut usize) -> Result<(), String> {
    if dict_name_equals(dict, b"/Subtype", b"/Image") {
        return Ok(());
    }

    let filters = match stream_filters(dict)? {
        None => {
            consume_budget(remaining, content.len())?;
            return Ok(());
        }
        Some(filters) => filters,
    };

    match filters.as_slice() {
        [] => consume_budget(remaining, content.len())?,
        [filter] if filter == b"FlateDecode" || filter == b"Fl" => {
            let expanded = bounded_flate_size(content, *remaining)?;
            consume_budget(remaining, expanded)?;
        }
        filters
            if filters.iter().any(|filter| {
                matches!(
                    filter.as_slice(),
                    b"LZWDecode" | b"LZW" | b"ASCII85Decode" | b"A85" | b"FlateDecode" | b"Fl"
                )
            }) =>
        {
            let chain = filters
                .iter()
                .map(|filter| String::from_utf8_lossy(filter))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "PDF stream filter chain '[{chain}]' is unsupported by bounded extraction"
            ));
        }
        // Image codecs and other filters are not expanded by the text path.
        _ => {}
    }
    Ok(())
}

fn stream_filters(dict: &[u8]) -> Result<Option<Vec<Vec<u8>>>, String> {
    let Some(value) = dict_value_after_name(dict, b"/Filter") else {
        return Ok(None);
    };
    let value = trim_pdf_whitespace(value);
    if let Some(name) = parse_pdf_name(value) {
        return Ok(Some(vec![name]));
    }
    if value.first() == Some(&b'[') {
        let mut filters = Vec::new();
        let mut index = 1usize;
        while index < value.len() {
            match value[index] {
                b']' => return Ok(Some(filters)),
                b'/' => {
                    let rest = &value[index..];
                    let Some(name) = parse_pdf_name(rest) else {
                        return Err("PDF stream has a malformed /Filter array".to_owned());
                    };
                    index += 1 + name.len();
                    filters.push(name);
                }
                byte if byte.is_ascii_whitespace() => index += 1,
                _ => return Err("PDF stream has a malformed /Filter array".to_owned()),
            }
        }
        return Err("PDF stream has a malformed /Filter array".to_owned());
    }
    Err("PDF stream has a malformed /Filter entry".to_owned())
}

fn dict_name_equals(dict: &[u8], key: &[u8], expected: &[u8]) -> bool {
    dict_value_after_name(dict, key)
        .and_then(parse_pdf_name)
        .is_some_and(|name| {
            let mut full = Vec::with_capacity(name.len() + 1);
            full.push(b'/');
            full.extend_from_slice(&name);
            full == expected
        })
}

fn dict_value_after_name<'a>(dict: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    dict.windows(name.len())
        .enumerate()
        .find_map(|(start, window)| {
            if window != name {
                return None;
            }
            let after_name = start + name.len();
            match dict.get(after_name) {
                None => None,
                Some(byte) if is_pdf_name_delimiter(*byte) => {
                    Some(trim_pdf_whitespace(&dict[after_name..]))
                }
                Some(_) => None,
            }
        })
}

fn parse_pdf_name(bytes: &[u8]) -> Option<Vec<u8>> {
    let bytes = trim_pdf_whitespace(bytes);
    if bytes.first() != Some(&b'/') {
        return None;
    }
    let mut name = Vec::new();
    let mut index = 1usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if is_pdf_name_delimiter(byte) {
            break;
        }
        name.push(byte);
        index += 1;
    }
    Some(name)
}

fn parse_pdf_u32_prefix(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let bytes = trim_pdf_whitespace(bytes);
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    let value = std::str::from_utf8(&bytes[..digits])
        .ok()
        .and_then(|text| text.parse::<u32>().ok())?;
    Some((value, &bytes[digits..]))
}

fn trim_pdf_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn is_pdf_name_delimiter(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b'/' | b'[' | b']' | b'(' | b')' | b'<' | b'>' | b'{' | b'}' | b'%'
        )
}

fn collect_direct_integer_objects(bytes: &[u8]) -> BTreeMap<u32, i64> {
    let mut values = BTreeMap::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let Some(object_number) = parse_object_header(bytes, index) else {
            index += 1;
            continue;
        };
        let Some(header_end) = find_object_header_end(bytes, index) else {
            index += 1;
            continue;
        };
        let body = trim_pdf_whitespace(&bytes[header_end..]);
        if let Some((value, rest)) = parse_pdf_i64_prefix(body) {
            let rest = trim_pdf_whitespace(rest);
            if rest.starts_with(b"endobj") {
                values.insert(object_number, value);
            }
        }
        index = header_end;
    }
    values
}

fn parse_object_header(bytes: &[u8], index: usize) -> Option<u32> {
    if index > 0 && !bytes[index - 1].is_ascii_whitespace() {
        return None;
    }
    let (object_number, rest) = parse_pdf_u32_prefix(&bytes[index..])?;
    let rest = trim_pdf_whitespace(rest);
    let (generation, rest) = parse_pdf_u32_prefix(rest)?;
    if generation != 0 {
        return None;
    }
    let rest = trim_pdf_whitespace(rest);
    if !rest.starts_with(b"obj") {
        return None;
    }
    let after = rest.get(3).copied();
    if after.is_some_and(|byte| !byte.is_ascii_whitespace() && byte != b'<' && byte != b'[') {
        return None;
    }
    Some(object_number)
}

fn find_object_header_end(bytes: &[u8], index: usize) -> Option<usize> {
    let (_, rest) = parse_pdf_u32_prefix(&bytes[index..])?;
    let rest = trim_pdf_whitespace(rest);
    let (_, rest) = parse_pdf_u32_prefix(rest)?;
    let rest = trim_pdf_whitespace(rest);
    if !rest.starts_with(b"obj") {
        return None;
    }
    Some(bytes.len() - rest.len() + 3)
}

fn parse_pdf_i64_prefix(bytes: &[u8]) -> Option<(i64, &[u8])> {
    let bytes = trim_pdf_whitespace(bytes);
    let mut index = 0usize;
    if bytes.first() == Some(&b'+') || bytes.first() == Some(&b'-') {
        index = 1;
    }
    let digits = bytes[index..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    let end = index + digits;
    let value = std::str::from_utf8(&bytes[..end])
        .ok()
        .and_then(|text| text.parse::<i64>().ok())?;
    Some((value, &bytes[end..]))
}

pub(in crate::document) fn consume_budget(
    remaining: &mut usize,
    size: usize,
) -> Result<(), String> {
    *remaining = remaining.checked_sub(size).ok_or_else(|| {
        format!("PDF expanded stream data exceeds the {MAX_PDF_EXPANDED_STREAM_BYTES} byte limit")
    })?;
    Ok(())
}

pub(in crate::document) fn bounded_flate_size(
    compressed: &[u8],
    remaining: usize,
) -> Result<usize, String> {
    let mut decoder = ZlibDecoder::new(compressed).take(remaining.saturating_add(1) as u64);
    let mut expanded = Vec::with_capacity(remaining.min(64 * 1024));
    decoder
        .read_to_end(&mut expanded)
        .map_err(|error| format!("could not validate PDF stream expansion: {error}"))?;
    if expanded.len() > remaining {
        return Err(format!(
            "PDF stream expands beyond its remaining {remaining} byte budget"
        ));
    }
    Ok(expanded.len())
}
