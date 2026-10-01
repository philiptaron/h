//! The shell function printed by `h-shell-init`.

use std::ffi::OsString;

use h_core::args::{Command, unknown_option};
use h_core::shell::{CdCommand, Shell};

pub const H_USAGE: &str = "Usage: eval \"$(h-shell-init [--pushd] [--name NAME] [--store DIR] \
                           [--git-opts \"OPTIONS\"] [code-root])\"";

/// Environment variable holding the default code root.
pub const CODE_ROOT_ENV: &str = "H_CODE_ROOT";
/// Code root used when neither an argument nor [`CODE_ROOT_ENV`] gives one.
pub const DEFAULT_CODE_ROOT: &str = "~/src";

/// The subcommands of `h store`. Only these send `h store` to the store commands, so a project
/// named `store` can still be jumped to with `h store`.
pub const STORE_COMMANDS: &[&str] =
    &["init", "add", "remove", "fetch", "list", "path", "remote", "show", "worktree", "maintain"];

/// Options accepted by `h-shell-init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HInitOptions {
    pub func_name: String,
    pub cd: CdCommand,
    pub git_opts: String,
    pub code_root: Option<OsString>,
    /// The object store clones borrow from and `h store` operates on.
    pub store: Option<OsString>,
}

impl Default for HInitOptions {
    fn default() -> Self {
        HInitOptions {
            func_name: "h".into(),
            cd: CdCommand::Cd,
            git_opts: String::new(),
            code_root: None,
            store: None,
        }
    }
}

/// Parse the arguments to `h-shell-init` (excluding the program name).
pub fn parse_h_init_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Command<HInitOptions>, String> {
    let mut opts = HInitOptions::default();
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        let has_value = args.peek().is_some();
        match arg.to_str() {
            Some("--pushd") => opts.cd = CdCommand::Pushd,
            Some("--name") if has_value => {
                opts.func_name = args.next().unwrap().to_string_lossy().into_owned()
            }
            Some("--git-opts") if has_value => {
                opts.git_opts = args.next().unwrap().to_string_lossy().into_owned()
            }
            Some("--store") if has_value => opts.store = args.next(),
            Some("-h" | "--help") => return Ok(Command::Help),
            Some("-V" | "--version") => return Ok(Command::Version),
            _ if !arg.as_encoded_bytes().starts_with(b"-") => opts.code_root = Some(arg),
            _ => return Err(unknown_option(&arg)),
        }
    }
    Ok(Command::Run(opts))
}

/// The shell function (and completion, for bash and zsh) that wraps `h`.
///
/// `h <term> [clone options]` runs `h go` and changes directory to what it prints; `h store
/// <command> ...` runs the store commands in place. Both pass the code root, the store and the git options.
pub fn render_h_init(
    opts: &HInitOptions,
    h_exe: &str,
    code_root: &str,
    store: Option<&str>,
    shell: Shell,
) -> String {
    let name = &opts.func_name;
    let cd = opts.cd.as_str();
    let mut common = format!("command {h_exe} --root \"{code_root}\"");
    if let Some(store) = store {
        common.push_str(&format!(" --store \"{store}\""));
    }
    let tail =
        if opts.git_opts.is_empty() { String::new() } else { format!(" -- {}", opts.git_opts) };
    let store_commands: Vec<String> =
        STORE_COMMANDS.iter().chain(&["-h", "--help"]).map(|c| format!("store:{c}")).collect();
    let store_commands = store_commands.join("|");
    let mut out = format!(
        "{name}() {{\n\
         \x20 case \"$1:$2\" in\n\
         \x20   {store_commands})\n\
         \x20     {common} \"$@\"{tail}\n\
         \x20     return\n\
         \x20     ;;\n\
         \x20 esac\n\
         \x20 _h_dir=$({common} go \"$@\"{tail})\n\
         \x20 _h_ret=$?\n\
         \x20 [ \"$_h_dir\" != \"$PWD\" ] && {cd} \"$_h_dir\"\n\
         \x20 return $_h_ret\n\
         }}\n"
    );

    // Only emit completion code for the detected shell: bash fails to parse zsh glob qualifiers
    // like `*(N/:t)` even inside an untaken branch of an `if`.
    match shell {
        Shell::Zsh => out.push_str(&format!(
            "_{name}_complete() {{\n\
             \x20 local code_root='{code_root}'\n\
             \x20 local -a projects\n\
             \x20 [[ -d \"$code_root\" ]] || return\n\
             \x20 projects=(\n\
             \x20   \"$code_root\"/*(N/:t)\n\
             \x20   \"$code_root\"/*/*(N/:t)\n\
             \x20   \"$code_root\"/*/*/*(N/:t)\n\
             \x20 )\n\
             \x20 projects=(\"${{(u)projects[@]}}\")\n\
             \x20 compadd -a projects\n\
             }}\n\
             compdef _{name}_complete {name}\n"
        )),
        Shell::Bash => out.push_str(&format!(
            "_{name}_complete() {{\n\
             \x20 local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n\
             \x20 local code_root='{code_root}'\n\
             \x20 COMPREPLY=()\n\
             \x20 [[ -d \"$code_root\" ]] || return\n\
             \x20 local dirs\n\
             \x20 dirs=$(find \"$code_root\" -mindepth 1 -maxdepth 3 -type d -not -name '.*' \
             2>/dev/null | sed 's|.*/||' | sort -u)\n\
             \x20 COMPREPLY=($(compgen -W \"$dirs\" -- \"$cur\"))\n\
             }}\n\
             complete -F _{name}_complete {name}\n"
        )),
        Shell::Unknown => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_h_init_defaults() {
        assert_eq!(parse_h_init_args(args(&[])), Ok(Command::Run(HInitOptions::default())));
    }

    #[test]
    fn parses_h_init_options() {
        let parsed = parse_h_init_args(args(&[
            "--pushd",
            "--name",
            "j",
            "--git-opts",
            "--depth 1",
            "--store",
            "~/store",
            "~/code",
        ]));
        assert_eq!(
            parsed,
            Ok(Command::Run(HInitOptions {
                func_name: "j".into(),
                cd: CdCommand::Pushd,
                git_opts: "--depth 1".into(),
                code_root: Some("~/code".into()),
                store: Some("~/store".into()),
            }))
        );
    }

    #[test]
    fn last_code_root_wins() {
        let Ok(Command::Run(opts)) = parse_h_init_args(args(&["a", "b"])) else { panic!() };
        assert_eq!(opts.code_root, Some("b".into()));
    }

    #[test]
    fn parses_h_init_help() {
        assert_eq!(parse_h_init_args(args(&["-h"])), Ok(Command::Help));
        assert_eq!(parse_h_init_args(args(&["--pushd", "--help", "--bogus"])), Ok(Command::Help));
    }

    #[test]
    fn parses_h_init_version() {
        assert_eq!(parse_h_init_args(args(&["-V"])), Ok(Command::Version));
        assert_eq!(parse_h_init_args(args(&["--version"])), Ok(Command::Version));
    }

    #[test]
    fn rejects_unknown_h_init_options() {
        assert_eq!(parse_h_init_args(args(&["--bogus"])), Err("Unknown option: --bogus".into()));
        assert_eq!(parse_h_init_args(args(&["-"])), Err("Unknown option: -".into()));
        // Options missing their value are unknown.
        assert_eq!(parse_h_init_args(args(&["--name"])), Err("Unknown option: --name".into()));
        assert_eq!(
            parse_h_init_args(args(&["--git-opts"])),
            Err("Unknown option: --git-opts".into())
        );
        assert_eq!(parse_h_init_args(args(&["--store"])), Err("Unknown option: --store".into()));
    }

    #[test]
    fn renders_h_function() {
        let out = render_h_init(&HInitOptions::default(), "/bin/h", "/code", None, Shell::Unknown);
        assert_eq!(
            out,
            r#"h() {
  case "$1:$2" in
    store:init|store:add|store:remove|store:fetch|store:list|store:path|store:remote|store:show|store:worktree|store:maintain|store:-h|store:--help)
      command /bin/h --root "/code" "$@"
      return
      ;;
  esac
  _h_dir=$(command /bin/h --root "/code" go "$@")
  _h_ret=$?
  [ "$_h_dir" != "$PWD" ] && cd "$_h_dir"
  return $_h_ret
}
"#
        );
    }

    #[test]
    fn renders_h_function_with_store_and_git_opts() {
        let opts = HInitOptions {
            func_name: "j".into(),
            cd: CdCommand::Pushd,
            git_opts: "-c user.name=\"Me Too\"".into(),
            code_root: None,
            store: Some("/store".into()),
        };
        let out = render_h_init(&opts, "/bin/h", "/code", Some("/store"), Shell::Unknown);
        assert_eq!(
            out,
            r#"j() {
  case "$1:$2" in
    store:init|store:add|store:remove|store:fetch|store:list|store:path|store:remote|store:show|store:worktree|store:maintain|store:-h|store:--help)
      command /bin/h --root "/code" --store "/store" "$@" -- -c user.name="Me Too"
      return
      ;;
  esac
  _h_dir=$(command /bin/h --root "/code" --store "/store" go "$@" -- -c user.name="Me Too")
  _h_ret=$?
  [ "$_h_dir" != "$PWD" ] && pushd "$_h_dir"
  return $_h_ret
}
"#
        );
    }

    #[test]
    fn renders_bash_completion() {
        let out = render_h_init(&HInitOptions::default(), "/bin/h", "/code", None, Shell::Bash);
        let completion = out.split_once("\n}\n").unwrap().1;
        assert_eq!(
            completion,
            r#"_h_complete() {
  local cur="${COMP_WORDS[COMP_CWORD]}"
  local code_root='/code'
  COMPREPLY=()
  [[ -d "$code_root" ]] || return
  local dirs
  dirs=$(find "$code_root" -mindepth 1 -maxdepth 3 -type d -not -name '.*' 2>/dev/null | sed 's|.*/||' | sort -u)
  COMPREPLY=($(compgen -W "$dirs" -- "$cur"))
}
complete -F _h_complete h
"#
        );
    }

    #[test]
    fn renders_zsh_completion() {
        let opts = HInitOptions { func_name: "j".into(), ..Default::default() };
        let out = render_h_init(&opts, "/bin/h", "/code", None, Shell::Zsh);
        let completion = out.split_once("\n}\n").unwrap().1;
        assert_eq!(
            completion,
            r#"_j_complete() {
  local code_root='/code'
  local -a projects
  [[ -d "$code_root" ]] || return
  projects=(
    "$code_root"/*(N/:t)
    "$code_root"/*/*(N/:t)
    "$code_root"/*/*/*(N/:t)
  )
  projects=("${(u)projects[@]}")
  compadd -a projects
}
compdef _j_complete j
"#
        );
    }
}
