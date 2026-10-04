use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    MonthDate,
    YearMonthDate,
    Date,
}

impl Layout {
    pub fn next(self) -> Self {
        match self {
            Self::MonthDate => Self::YearMonthDate,
            Self::YearMonthDate => Self::Date,
            Self::Date => Self::MonthDate,
        }
    }
    pub fn directory(self, date: NaiveDate) -> PathBuf {
        PathBuf::from(
            date.format(match self {
                Self::MonthDate => "%m/%Y-%m-%d",
                Self::YearMonthDate => "%Y/%m/%Y-%m-%d",
                Self::Date => "%Y-%m-%d",
            })
            .to_string(),
        )
    }
}

impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MonthDate => "Month / date",
            Self::YearMonthDate => "Year / month / date",
            Self::Date => "Date only",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    #[default]
    Separate,
    Subfolders,
    Together,
}

impl Grouping {
    pub fn next(self) -> Self {
        match self {
            Self::Separate => Self::Subfolders,
            Self::Subfolders => Self::Together,
            Self::Together => Self::Separate,
        }
    }
}

impl fmt::Display for Grouping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Separate => "Date-RAW / date-JPEG",
            Self::Subfolders => "Date / RAW and JPEG",
            Self::Together => "Together in date folder",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DateBasis {
    #[default]
    Capture,
    Import,
}

impl fmt::Display for DateBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Capture => "Capture date (EXIF)",
            Self::Import => "Import date (local)",
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub archive: PathBuf,
    pub layout: Layout,
    pub grouping: Grouping,
    pub date_basis: DateBasis,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            archive: dirs::picture_dir()
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Photo Archive"),
            layout: Layout::default(),
            grouping: Grouping::default(),
            date_basis: DateBasis::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Some(
                toml::from_str(&text).context("Invalid shotctl configuration")?,
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("Cannot read shotctl configuration"),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if !self.archive.is_absolute() {
            bail!("Choose an absolute archive path (for example ~/Pictures/Archive)");
        }
        fs::create_dir_all(&self.archive)
            .with_context(|| format!("Cannot create archive {}", self.archive.display()))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut pending = tempfile::NamedTempFile::new_in(parent)?;
        pending.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        pending.as_file().sync_all()?;
        pending
            .persist(path)
            .map_err(|error| error.error)
            .context("Cannot save configuration")?;
        Ok(())
    }

    pub fn directory(&self, date: NaiveDate, kind: &str) -> PathBuf {
        let mut relative = self.layout.directory(date);
        match self.grouping {
            Grouping::Separate => {
                let leaf = relative.file_name().unwrap().to_string_lossy();
                relative.set_file_name(format!("{leaf}-{kind}"));
            }
            Grouping::Subfolders => relative.push(kind),
            Grouping::Together => {}
        }
        self.archive.join(relative)
    }

    pub fn example(&self) -> String {
        self.directory(NaiveDate::from_ymd_opt(2026, 10, 22).unwrap(), "RAW")
            .display()
            .to_string()
    }
}

pub fn config_path() -> Result<PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(dirs::config_dir)
        .context("Cannot determine configuration directory")?;
    Ok(root.join("shotctl/config.toml"))
}

pub fn expand_path(input: &str) -> PathBuf {
    let input = input.trim();
    if input == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(input));
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return dirs::home_dir().unwrap_or_default().join(rest);
    }
    PathBuf::from(input)
}
