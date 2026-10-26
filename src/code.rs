//! Transfer Codes: 5 random characters from base62 minus the look-alikes `0 O 1 l I`.

pub const LEN: usize = 5;
pub const ALPHABET: &[u8; 57] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Draws a code from the OS CSPRNG; rejection sampling keeps the distribution uniform.
pub fn generate() -> String {
    // 228 = 4 * 57, the largest multiple of 57 that fits in a byte.
    const LIMIT: u8 = 228;
    let mut out = String::with_capacity(LEN);
    let mut buf = [0u8; 16];
    while out.len() < LEN {
        getrandom::fill(&mut buf).expect("OS random number generator unavailable");
        for &b in buf.iter().filter(|&&b| b < LIMIT) {
            if out.len() == LEN {
                break;
            }
            out.push(ALPHABET[(b % 57) as usize] as char);
        }
    }
    out
}

pub fn is_well_formed(s: &str) -> bool {
    s.len() == LEN && s.bytes().all(|b| ALPHABET.contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_has_no_lookalikes() {
        for c in b"0O1lI" {
            assert!(!ALPHABET.contains(c));
        }
        let mut sorted = ALPHABET.to_vec();
        sorted.dedup();
        assert_eq!(sorted.len(), 57);
    }

    #[test]
    fn generated_codes_are_well_formed() {
        for _ in 0..1000 {
            assert!(is_well_formed(&generate()));
        }
        assert!(!is_well_formed("Ak9z"));
        assert!(!is_well_formed("Ak9z0"));
    }
}
