use crate::VERSION;

use ajour_core::{config::Flavor, repository::CompressionFormat};

use isahc::http::Uri;
use structopt::{
    clap::{self, AppSettings},
    StructOpt,
};

use std::env;
use std::path::PathBuf;

pub fn get_opts() -> Result<Opts, clap::Error> {
    let args = env::args_os();

    Opts::from_iter_safe(args)
}

#[allow(unused_variables)]
pub fn validate_opts_or_exit(
    opts_result: Result<Opts, clap::Error>,
    is_cli: bool,
    is_debug: bool,
) -> Opts {
    // If an error, we need to setup the AttachConsole fix for Windows release
    // so we can exit and display the error message to the user.
    let is_opts_error = opts_result.is_err();

    // Workaround to output to console even though we compile with windows_subsystem = "windows"
    // in release mode
    #[cfg(target_os = "windows")]
    {
        if (is_cli || is_opts_error) && !is_debug {
            use winapi::um::wincon::{AttachConsole, ATTACH_PARENT_PROCESS};

            unsafe {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }

    // Now that the windows fix is successfully setup, we can safely exit on the error
    // and it will display properly to the user, or carry forward with the program
    // and properly display our logging to the user. Since `e.exit()` returns a `!`,
    // we can return the Ok(Opts) and carry forward with our program.
    match opts_result {
        Ok(opts) => opts,
        Err(e) => {
            // Apparently on `--version`, there is no "error message" that gets displayed
            // like with `--help` and actual clap errors. It gets printed before we
            // ever hit our console fix for windows, so let's manually print it
            // before exiting
            #[cfg(target_os = "windows")]
            {
                if !is_debug && e.kind == clap::ErrorKind::VersionDisplayed {
                    print!("Ajour {}", VERSION);
                }
            }

            e.exit();
        }
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "Ajour",
            about = env!("CARGO_PKG_DESCRIPTION"),
            version = VERSION,
            author = env!("CARGO_PKG_AUTHORS"),
            setting = AppSettings::DisableHelpSubcommand)]
pub struct Opts {
    // Modified 2026-10-28; original GPLv3 applies.
    #[structopt(
        long,
        parse(from_os_str),
        help = "Open a copied WoW folder for inventory and recovery review; restoring originals requires confirmation"
    )]
    pub preview: Option<PathBuf>,
    #[structopt(
        long,
        requires = "preview",
        help = "Check preview data without opening a window"
    )]
    pub preview_check: bool,
    #[structopt(
        long,
        hidden = true,
        requires = "preview",
        help = "Check hidden native preview startup and exit"
    )]
    pub preview_window_check: bool,
    // Added 2026-10-28; original GPLv3 applies.
    #[structopt(
        long,
        hidden = true,
        help = "Check safe startup without opening a window"
    )]
    pub startup_check: bool,

    #[structopt(long = "data", help = "Path to a custom data directory for the app")]
    pub data_directory: Option<PathBuf>,
    #[structopt(long = "aa", help = "Enable / Disable Anti-aliasing (true / false)")]
    pub antialiasing: Option<bool>,
    #[structopt(subcommand)]
    pub command: Option<Command>,
    #[structopt(long, hidden = true)]
    pub self_update_temp: Option<PathBuf>,
}

// Added 2026-10-28; original GPLv3 applies.
impl Opts {
    pub fn uses_safe_startup(&self) -> bool {
        self.command.is_none()
            || self.preview.is_some()
            || self.preview_check
            || self.preview_window_check
            || self.startup_check
    }
}

#[derive(Debug, StructOpt)]
pub enum Command {
    /// Update all addons, WeakAura and Plater auras
    Update,
    /// Update all addons
    UpdateAddons,
    /// Update all WeakAura and Plater auras
    UpdateAuras,
    /// Install an addon
    Install {
        #[structopt(parse(try_from_str = str_to_flavor), possible_values = &["retail","ptr","beta","classic_tbc","classic_era","classic_era_ptr","classic_ptr","classic_beta"])]
        /// flavor to install addon under
        flavor: Flavor,
        #[structopt()]
        /// source url [Github & Gitlab currently supported]
        url: Uri,
    },
    /// Backup your WTF and/or AddOns folders
    Backup {
        #[structopt(short, long, default_value = "all", parse(try_from_str = str_to_backup_folder), possible_values = &["all","wtf","addons","config", "screenshots", "fonts"])]
        /// folder to backup
        backup_folder: BackupFolder,
        #[structopt(short, long, parse(try_from_str = str_to_flavor), possible_values = &["retail","ptr","beta","classic_tbc","classic_era","classic_era_ptr","classic_ptr","classic_beta"])]
        /// space separated list of flavors to include in backup. If ommited, all flavors will be included.
        flavors: Vec<Flavor>,
        #[structopt()]
        /// folder to save backups to
        destination: PathBuf,
        #[structopt(short, long, default_value = "zip", possible_values = &["zip", "zstd"])]
        compression_format: CompressionFormat,
        #[structopt(short, long, default_value = "0")]
        level: i32,
    },
    /// Add a World of Warcraft path
    PathAdd {
        /// path to the World of Warcraft directory
        path: PathBuf,
        #[structopt(parse(try_from_str = str_to_flavor), possible_values = &["retail","ptr","beta","classic_era","classic_era_ptr","classic","classic_ptr","classic_beta"])]
        /// flavor to use from the path. If none, we use all we find
        flavor: Option<Flavor>,
    },
}

fn str_to_flavor(s: &str) -> Result<Flavor, &'static str> {
    match s {
        "retail" => Ok(Flavor::Retail),
        "beta" => Ok(Flavor::RetailBeta),
        "ptr" => Ok(Flavor::RetailPtr),
        "classic_tbc" => Ok(Flavor::ClassicTbc),
        "classic_era" => Ok(Flavor::ClassicEra),
        "classic_era_ptr" => Ok(Flavor::ClassicEraPtr),
        "classic_ptr" => Ok(Flavor::ClassicPtr),
        "classic_beta" => Ok(Flavor::ClassicBeta),
        _ => Err("valid values are ['retail','ptr','beta','classic_tbc','classic_era','classic_era_ptr','classic_ptr','classic_beta']"),
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BackupFolder {
    All,
    AddOns,
    Wtf,
    Config,
    Screenshots,
    Fonts,
}

fn str_to_backup_folder(s: &str) -> Result<BackupFolder, &'static str> {
    match s {
        "all" => Ok(BackupFolder::All),
        "wtf" => Ok(BackupFolder::Wtf),
        "addons" => Ok(BackupFolder::AddOns),
        "config" => Ok(BackupFolder::Config),
        "screenshots" => Ok(BackupFolder::Screenshots),
        "fonts" => Ok(BackupFolder::Fonts),
        _ => Err("valid values are ['all','wtf','addons','config','screenshots','fonts']"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_check_flags_require_an_explicit_path() {
        for flag in ["--preview-check", "--preview-window-check"] {
            assert!(Opts::from_iter_safe(["ajour", flag]).is_err());
        }
    }

    // Added 2026-10-28; original GPLv3 applies.
    #[test]
    fn no_arguments_and_gui_options_use_the_actual_safe_entry_decision() {
        let opts = Opts::from_iter_safe(["ajour"]).unwrap();
        assert!(opts.command.is_none());
        assert!(opts.preview.is_none());
        assert!(!opts.preview_check);
        assert!(!opts.preview_window_check);
        assert!(!opts.startup_check);
        assert!(opts.uses_safe_startup());

        for args in [
            vec!["ajour", "--aa", "true"],
            vec!["ajour", "--aa", "false"],
            vec!["ajour", "--data", "isolated-data"],
            vec!["ajour", "--self-update-temp", "cleanup-sentinel"],
            vec!["ajour", "--startup-check"],
            vec!["ajour", "--preview", "copied-wow"],
        ] {
            let opts = Opts::from_iter_safe(args).unwrap();
            assert!(opts.command.is_none());
            assert!(opts.uses_safe_startup());
        }
    }

    #[test]
    fn startup_check_does_not_relax_existing_explicit_path_requirements() {
        let opts = Opts::from_iter_safe(["ajour", "--startup-check"]).unwrap();
        assert!(opts.startup_check);
        assert!(opts.preview.is_none());
        assert!(opts.uses_safe_startup());

        for flag in ["--preview-check", "--preview-window-check"] {
            assert!(Opts::from_iter_safe(["ajour", "--startup-check", flag]).is_err());
            let opts = Opts::from_iter_safe(["ajour", "--preview", "copied-wow", flag]).unwrap();
            assert!(opts.preview.is_some());
            assert!(opts.uses_safe_startup());
        }

        let opts = Opts::from_iter_safe(["ajour", "--preview", "", "--startup-check"]).unwrap();
        assert_eq!(opts.preview, Some(PathBuf::new()));
        assert!(opts.uses_safe_startup());
    }

    #[test]
    fn explicit_legacy_subcommands_still_parse_without_selecting_safe_gui() {
        for (name, expected) in [("update", 0), ("update-addons", 1), ("update-auras", 2)] {
            let opts = Opts::from_iter_safe(["ajour", name]).unwrap();
            assert!(!opts.uses_safe_startup());
            match (expected, opts.command.unwrap()) {
                (0, Command::Update) | (1, Command::UpdateAddons) | (2, Command::UpdateAuras) => {}
                other => panic!("Unexpected command: {:?}", other),
            }
        }

        let opts = Opts::from_iter_safe([
            "ajour",
            "install",
            "retail",
            "https://invalid.invalid/fixture",
        ])
        .unwrap();
        assert!(!opts.uses_safe_startup());
        match opts.command.unwrap() {
            Command::Install { flavor, url } => {
                assert!(matches!(flavor, Flavor::Retail));
                assert_eq!(url.to_string(), "https://invalid.invalid/fixture");
            }
            other => panic!("Unexpected command: {:?}", other),
        }

        let opts = Opts::from_iter_safe(["ajour", "backup", "copied-backup"]).unwrap();
        assert!(!opts.uses_safe_startup());
        match opts.command.unwrap() {
            Command::Backup {
                backup_folder,
                destination,
                flavors,
                level,
                ..
            } => {
                assert!(matches!(backup_folder, BackupFolder::All));
                assert_eq!(destination, PathBuf::from("copied-backup"));
                assert!(flavors.is_empty());
                assert_eq!(level, 0);
            }
            other => panic!("Unexpected command: {:?}", other),
        }

        let opts = Opts::from_iter_safe(["ajour", "path-add", "copied-wow", "retail"]).unwrap();
        assert!(!opts.uses_safe_startup());
        match opts.command.unwrap() {
            Command::PathAdd { path, flavor } => {
                assert_eq!(path, PathBuf::from("copied-wow"));
                assert!(matches!(flavor, Some(Flavor::Retail)));
            }
            other => panic!("Unexpected command: {:?}", other),
        }

        let opts = Opts::from_iter_safe(["ajour", "--data", "isolated-data", "update"]).unwrap();
        assert_eq!(opts.data_directory, Some(PathBuf::from("isolated-data")));
        assert!(!opts.uses_safe_startup());
    }

    #[test]
    fn mixed_preview_and_command_options_route_to_rejection_not_command_execution() {
        for args in [
            vec!["ajour", "--startup-check", "update"],
            vec![
                "ajour",
                "--preview",
                "copied-wow",
                "--startup-check",
                "update",
            ],
            vec![
                "ajour",
                "--preview",
                "copied-wow",
                "--preview-check",
                "update",
            ],
        ] {
            let opts = Opts::from_iter_safe(args).unwrap();
            assert!(opts.command.is_some());
            assert!(opts.uses_safe_startup());

            // Call only the isolated entry. Never dispatch the CLI command.
            // Every invocation is headless even if command rejection regresses.
            let error = crate::inventory_preview::run(&opts).unwrap_err();
            if cfg!(feature = "no-self-update") {
                assert!(error.to_string().contains("legacy commands"));
            } else {
                assert!(error.to_string().contains("no-self-update"));
            }
        }
    }
}
