use crate::csv_fetcher::Member;
use chrono::Utc;

pub struct NotesGenerator {
    emoji: String,
}

impl NotesGenerator {
    pub fn new(emoji: String) -> Self {
        Self { emoji }
    }

    pub fn generate(&self, members: &[Member]) -> String {
        let mut output = String::new();

        // Header comments
        output.push_str("# QRQ Crew Callsign Notes for Ham2K PoLo\n");
        output.push_str(&format!(
            "# Generated: {}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));
        output.push_str("# https://qrqcrew.club\n");
        output.push_str("# Do not edit manually - this file is auto-generated\n");
        output.push('\n');

        // Sort and generate entries
        let mut sorted: Vec<_> = members.iter().collect();
        sorted.sort_by(|a, b| a.callsign.cmp(&b.callsign));

        for member in sorted {
            output.push_str(&format!(
                "{} {} QRQ Crew #{}\n",
                member.callsign, self.emoji, member.qc_number
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
        let generator = NotesGenerator::new("⚓".to_string());

        let members = vec![
            Member {
                callsign: "W6JSV".to_string(),
                qc_number: 10,
            },
            Member {
                callsign: "K4MW".to_string(),
                qc_number: 1,
            },
            Member {
                callsign: "WN7JT".to_string(),
                qc_number: 2,
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
    fn test_generate_empty() {
        let generator = NotesGenerator::new("⚓".to_string());
        let output = generator.generate(&[]);

        // Should still have header
        assert!(output.contains("# QRQ Crew Callsign Notes"));

        // No callsign entries
        let entries: Vec<&str> = output
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
            .collect();
        assert!(entries.is_empty());
    }
}
