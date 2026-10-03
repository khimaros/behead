//! just enough x.509 parsing to name a certificate in the log: subject,
//! issuer and validity dates. anything unexpected yields None.

const VERSION_TAG: u8 = 0xa0;
const UTC_TIME_TAG: u8 = 0x17;
/// distinguished name attributes, by the last byte of their 2.5.4.x identifier
const ATTRIBUTE_PREFIX: [u8; 2] = [0x55, 0x04];
const ATTRIBUTES: [(u8, &str); 6] = [(6, "C"), (8, "ST"), (7, "L"), (10, "O"), (11, "OU"), (3, "CN")];
/// two digit years below this are 20xx, the rest 19xx, as rfc 5280 says
const UTC_CENTURY_PIVOT: u32 = 50;

/// the first der element in `data`: (tag, contents, what follows)
fn element(data: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = data.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (first as usize, rest)
    } else {
        let count = (first & 0x7f) as usize;
        let bytes = rest.get(..count).filter(|_| count <= 4)?;
        (bytes.iter().fold(0, |length, &byte| length << 8 | byte as usize), &rest[count..])
    };
    rest.get(..length).map(|contents| (tag, contents, &rest[length..]))
}

/// the elements inside a constructed der element's contents
fn children(mut data: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    std::iter::from_fn(move || {
        let (tag, contents, rest) = element(data)?;
        data = rest;
        Some((tag, contents))
    })
}

/// a distinguished name as `C=US/O=Example/CN=host`
fn name(der: &[u8]) -> String {
    let attributes = children(der).flat_map(|(_, set)| children(set)).filter_map(|(_, pair)| {
        let mut parts = children(pair);
        let ((_, oid), (_, value)) = (parts.next()?, parts.next()?);
        let id = oid.strip_prefix(&ATTRIBUTE_PREFIX[..]).filter(|id| id.len() == 1)?[0];
        let key = ATTRIBUTES.iter().find(|(known, _)| *known == id)?.1;
        Some(format!("{key}={}", String::from_utf8_lossy(value)))
    });
    attributes.collect::<Vec<_>>().join("/")
}

/// a utc or generalized time as YYYY-MM-DD
fn date(tag: u8, value: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(value).ok()?;
    let (year, rest) = if tag == UTC_TIME_TAG {
        let year: u32 = text.get(..2)?.parse().ok()?;
        (if year < UTC_CENTURY_PIVOT { 2000 + year } else { 1900 + year }, text.get(2..)?)
    } else {
        (text.get(..4)?.parse().ok()?, text.get(4..)?)
    };
    Some(format!("{year}-{}-{}", rest.get(..2)?, rest.get(2..4)?))
}

/// `subject ..., issuer ..., valid FROM to UNTIL` for a der certificate
pub fn describe(der: &[u8]) -> Option<String> {
    let (_, certificate, _) = element(der)?;
    let (_, tbs) = children(certificate).next()?;
    // the version field is absent from v1 certificates
    let mut fields = children(tbs).skip_while(|(tag, _)| *tag == VERSION_TAG).skip(2);
    let ((_, issuer), (_, validity), (_, subject)) = (fields.next()?, fields.next()?, fields.next()?);
    let mut dates = children(validity).map(|(tag, value)| date(tag, value));
    let (from, until) = (dates.next()??, dates.next()??);
    Some(format!("subject {}, issuer {}, valid {from} to {until}", name(subject), name(issuer)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_time_formats() {
        assert_eq!(date(UTC_TIME_TAG, b"140704000000Z").as_deref(), Some("2014-07-04"));
        assert_eq!(date(UTC_TIME_TAG, b"991231235959Z").as_deref(), Some("1999-12-31"));
        assert_eq!(date(0x18, b"20480801172123Z").as_deref(), Some("2048-08-01"));
    }

    #[test]
    fn reads_long_form_lengths_and_rejects_truncation() {
        let long = [&[0x04, 0x81, 0x80][..], &[7; 0x80]].concat();
        assert_eq!(element(&long).map(|(tag, contents, rest)| (tag, contents.len(), rest.len())), Some((4, 0x80, 0)));
        assert_eq!(element(&long[..10]), None);
        assert_eq!(describe(b"not a certificate"), None);
    }
}
