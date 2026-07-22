use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Which build of a package bakery follows: the tagged stable release, a
/// deliberately-promoted beta, or the continuously-published `dev` branch
/// build. Not to be confused with the *distribution* channel (bakery vs.
/// pacman) documented in `docs/release-channels.md` — that's an orthogonal,
/// pre-existing use of the word "channel", which is why this is called a
/// "track" instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Stable,
    Beta,
    Dev,
}

impl Default for Track {
    fn default() -> Self {
        Track::Stable
    }
}

impl Track {
    pub fn as_str(&self) -> &'static str {
        match self {
            Track::Stable => "stable",
            Track::Beta => "beta",
            Track::Dev => "dev",
        }
    }
}

impl fmt::Display for Track {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Track {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stable" => Ok(Track::Stable),
            "beta" => Ok(Track::Beta),
            "dev" => Ok(Track::Dev),
            other => Err(format!("unknown track '{other}' — expected stable, beta, or dev")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_stable() {
        assert_eq!(Track::default(), Track::Stable);
    }

    #[test]
    fn display_roundtrips_through_from_str() {
        for track in [Track::Stable, Track::Beta, Track::Dev] {
            let s = track.to_string();
            assert_eq!(s.parse::<Track>().unwrap(), track);
        }
    }

    #[test]
    fn from_str_is_case_insensitive() {
        assert_eq!("DEV".parse::<Track>().unwrap(), Track::Dev);
        assert_eq!("Beta".parse::<Track>().unwrap(), Track::Beta);
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!("nightly".parse::<Track>().is_err());
    }

    #[test]
    fn json_roundtrip_uses_lowercase() {
        let json = serde_json::to_string(&Track::Dev).unwrap();
        assert_eq!(json, "\"dev\"");
        let back: Track = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Track::Dev);
    }
}
