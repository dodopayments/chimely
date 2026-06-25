//! TypeIDs: `<prefix>_<uuidv7 as 26-char Crockford base32>`. The prefix is
//! constant per table and never stored. The database holds the raw uuid.
//! Mirrors the SQL helpers `typeid_format`/`typeid_parse`.

use uuid::Uuid;

pub const ENVIRONMENT: &str = "env";
pub const API_KEY: &str = "key";
pub const SUBSCRIBER: &str = "sub";
pub const NOTIFICATION: &str = "notif";
pub const BROADCAST: &str = "bcast";
pub const JOB: &str = "job";
pub const ADMIN_USER: &str = "adm";

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

pub fn new_uuid() -> Uuid {
    Uuid::now_v7()
}

/// `typeid("notif", uuid)` → `notif_01h455vb4pex5vsknk084sn02q`.
pub fn typeid(prefix: &str, id: Uuid) -> String {
    let bytes = id.as_bytes();
    let mut out = String::with_capacity(prefix.len() + 27);
    out.push_str(prefix);
    out.push('_');
    // 128 bits left-padded with 2 zero bits to 130 = 26 * 5.
    let mut acc: u32 = 0;
    let mut bits: u32 = 2;
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// Strict parse: the prefix must match exactly and the suffix must be 26
/// canonical (lowercase) Crockford base32 chars with the top 2 bits zero.
pub fn parse_typeid(prefix: &str, s: &str) -> Option<Uuid> {
    let suffix = s.strip_prefix(prefix)?.strip_prefix('_')?;
    if suffix.len() != 26 {
        return None;
    }
    let mut vals = suffix.bytes().map(decode_char);
    let first = vals.next()??;
    if first > 7 {
        return None; // would overflow 128 bits
    }
    let mut acc: u32 = u32::from(first);
    let mut bits: u32 = 3;
    let mut bytes = [0u8; 16];
    let mut i = 0;
    for v in vals {
        acc = (acc << 5) | u32::from(v?);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes[i] = ((acc >> bits) & 255) as u8;
            acc &= (1 << bits) - 1;
            i += 1;
        }
    }
    debug_assert_eq!(i, 16);
    Some(Uuid::from_bytes(bytes))
}

fn decode_char(c: u8) -> Option<u8> {
    ALPHABET.iter().position(|&a| a == c).map(|p| p as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Canonical vector from the TypeID spec (also asserted against the SQL
    // helpers in the migration tests).
    const SPEC_UUID: &str = "01890a5d-ac96-774b-bcce-b302099a8057";
    const SPEC_TYPEID: &str = "notif_01h455vb4pex5vsknk084sn02q";

    #[test]
    fn formats_the_spec_vector() {
        let id: Uuid = SPEC_UUID.parse().unwrap();
        assert_eq!(typeid(NOTIFICATION, id), SPEC_TYPEID);
    }

    #[test]
    fn parses_the_spec_vector() {
        assert_eq!(
            parse_typeid(NOTIFICATION, SPEC_TYPEID),
            Some(SPEC_UUID.parse().unwrap())
        );
    }

    #[test]
    fn round_trips_random_uuids() {
        for _ in 0..1000 {
            let id = new_uuid();
            assert_eq!(parse_typeid(BROADCAST, &typeid(BROADCAST, id)), Some(id));
        }
    }

    #[test]
    fn rejects_malformed_ids() {
        assert_eq!(
            parse_typeid(NOTIFICATION, "bcast_01h455vb4pex5vsknk084sn02q"),
            None
        );
        assert_eq!(
            parse_typeid(NOTIFICATION, "notif_01h455vb4pex5vsknk084sn02"),
            None
        ); // 25 chars
        assert_eq!(
            parse_typeid(NOTIFICATION, "notif_81h455vb4pex5vsknk084sn02q"),
            None
        ); // >7 lead
        assert_eq!(
            parse_typeid(NOTIFICATION, "notif_01H455VB4PEX5VSKNK084SN02Q"),
            None
        ); // uppercase
        assert_eq!(
            parse_typeid(NOTIFICATION, "notif_01h455vb4pex5vsknk084sn0il"),
            None
        ); // i/l
        assert_eq!(parse_typeid(NOTIFICATION, ""), None);
    }
}
