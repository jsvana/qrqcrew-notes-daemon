use crate::csv_fetcher::Member;
use chrono::Utc;

pub struct NotesGenerator {
    emoji: String,
    label: String,
    url: String,
}

impl NotesGenerator {
    pub fn new(emoji: String, label: String, url: Option<String>) -> Self {
        Self {
            emoji,
            label,
            url: url.unwrap_or_default(),
        }
    }

    pub fn generate(&self, members: &[Member]) -> String {
        let mut output = String::new();

        // Header comments
        output.push_str(&format!("# {} Callsign Notes for Ham2K PoLo\n", self.label));
        output.push_str(&format!(
            "# Generated: {}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));
        if !self.url.is_empty() {
            output.push_str(&format!("# {}\n", self.url));
        }
        output.push_str("# Do not edit manually - this file is auto-generated\n");
        output.push('\n');

        // Sort and generate entries
        let mut sorted: Vec<_> = members.iter().collect();
        sorted.sort_by(|a, b| a.callsign.cmp(&b.callsign));

        for member in sorted {
            output.push_str(&format!(
                "{} {} {} #{}\n",
                member.callsign, self.emoji, self.label, member.member_id
            ));
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_notes() {
        let generator = NotesGenerator::new(
            "⚓".to_string(),
            "QRQ Crew".to_string(),
            Some("https://qrqcrew.club".to_string()),
        );

        let members = vec![
            Member {
                callsign: "W6JSV".to_string(),
                member_id: "10".to_string(),
            },
            Member {
                callsign: "K4MW".to_string(),
                member_id: "1".to_string(),
            },
            Member {
                callsign: "WN7JT".to_string(),
                member_id: "2".to_string(),
            },
        ];

        let output = generator.generate(&members);

        // Check header
        assert!(output.contains("# QRQ Crew Callsign Notes for Ham2K PoLo"));
        assert!(output.contains("# Generated:"));
        assert!(output.contains("# https://qrqcrew.club"));

        // Check entries are sorted by callsign
        let lines: Vec<&str> = output
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "K4MW ⚓ QRQ Crew #1");
        assert_eq!(lines[1], "W6JSV ⚓ QRQ Crew #10");
        assert_eq!(lines[2], "WN7JT ⚓ QRQ Crew #2");
    }

    #[test]
    fn test_generate_cwops_format() {
        let generator = NotesGenerator::new(
            "🎹".to_string(),
            "CWops".to_string(),
            Some("https://cwops.org".to_string()),
        );

        let members = vec![Member {
            callsign: "W6JSV".to_string(),
            member_id: "1234".to_string(),
        }];

        let output = generator.generate(&members);

        assert!(output.contains("# CWops Callsign Notes for Ham2K PoLo"));
        assert!(output.contains("W6JSV 🎹 CWops #1234"));
    }

    #[test]
    fn test_generate_empty() {
        let generator = NotesGenerator::new("⚓".to_string(), "Test".to_string(), None);
        let output = generator.generate(&[]);

        assert!(output.contains("# Test Callsign Notes"));
        assert!(!output.contains("# https://")); // No URL when None

        let entries: Vec<&str> = output
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();
        assert!(entries.is_empty());
    }
}
