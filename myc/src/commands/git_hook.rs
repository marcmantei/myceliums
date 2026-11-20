//! `myc git-hook`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const HOOK_BEGIN: &str = "# BEGIN myceliums";
const HOOK_END: &str = "# END myceliums";

fn post_commit_script(myc_path: &str) -> String {
    format!(
        "# BEGIN myceliums\n\
         _myc_changed=$(git diff --name-only HEAD~1 2>/dev/null)\n\
         if [ -n \"$_myc_changed\" ]; then\n\
             {} analyze . --skip-embeddings >/dev/null 2>&1 &\n\
         fi\n\
         # END myceliums",
        myc_path
    )
}

fn post_checkout_script(myc_path: &str) -> String {
    format!(
        "# BEGIN myceliums\n\
         if [ \"$3\" = \"1\" ]; then\n\
             {} analyze . >/dev/null 2>&1 &\n\
         fi\n\
         # END myceliums",
        myc_path
    )
}

fn find_git_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let mut dir = cwd.as_path();
    loop {
        let git = dir.join(".git");
        if git.is_dir() {
            return Ok(git);
        }
        dir = dir
            .parent()
            .context("Not inside a git repository (no .git directory found)")?;
    }
}

fn install_hook(hook_path: &Path, script_block: &str) -> Result<()> {
    if hook_path.exists() {
        let existing = std::fs::read_to_string(hook_path)?;
        let new_content = if existing.contains(HOOK_BEGIN) {
            replace_hook_block(&existing, script_block)
        } else {
            let sep = if existing.ends_with('\n') { "" } else { "\n" };
            format!("{}{}{}\n", existing, sep, script_block)
        };
        std::fs::write(hook_path, new_content)?;
    } else {
        std::fs::write(hook_path, format!("#!/bin/sh\n{}\n", script_block))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(hook_path)?.permissions();
        perms.set_mode(perms.mode() | 0o111);
        std::fs::set_permissions(hook_path, perms)?;
    }

    Ok(())
}

fn replace_hook_block(existing: &str, new_block: &str) -> String {
    if let (Some(start), Some(end_pos)) = (existing.find(HOOK_BEGIN), existing.find(HOOK_END)) {
        let end = end_pos + HOOK_END.len();
        format!("{}{}{}", &existing[..start], new_block, &existing[end..])
    } else {
        format!("{}\n{}\n", existing, new_block)
    }
}

fn remove_hook(hook_path: &Path) -> Result<()> {
    if !hook_path.exists() {
        return Ok(());
    }
    let existing = std::fs::read_to_string(hook_path)?;
    if !existing.contains(HOOK_BEGIN) {
        return Ok(());
    }
    let new_content = remove_hook_block(&existing);
    let trimmed = new_content.trim();
    if trimmed.is_empty() || trimmed == "#!/bin/sh" {
        std::fs::remove_file(hook_path)?;
    } else {
        std::fs::write(hook_path, new_content)?;
    }
    Ok(())
}

fn remove_hook_block(content: &str) -> String {
    if let (Some(start), Some(end_pos)) = (content.find(HOOK_BEGIN), content.find(HOOK_END)) {
        let end = end_pos + HOOK_END.len();
        let before = content[..start].trim_end_matches('\n');
        let after = content[end..].trim_start_matches('\n');
        if before.is_empty() {
            format!("{}\n", after)
        } else if after.is_empty() {
            format!("{}\n", before)
        } else {
            format!("{}\n{}\n", before, after)
        }
    } else {
        content.to_string()
    }
}

pub(crate) fn cmd_hook_install() -> Result<()> {
    let git_dir = find_git_dir()?;
    let hooks_dir = git_dir.join("hooks");
    std::fs::create_dir_all(&hooks_dir)?;

    let myc_path = std::env::current_exe()
        .context("Could not determine myc binary path")?
        .to_string_lossy()
        .to_string();

    install_hook(
        &hooks_dir.join("post-commit"),
        &post_commit_script(&myc_path),
    )?;
    install_hook(
        &hooks_dir.join("post-checkout"),
        &post_checkout_script(&myc_path),
    )?;

    println!("  \u{2713} post-commit hook installed");
    println!("  \u{2713} post-checkout hook installed");
    println!();
    println!(
        "  The knowledge graph will rebuild automatically after each commit or branch switch."
    );
    println!("  To remove: myc hook uninstall");
    Ok(())
}

pub(crate) fn cmd_hook_uninstall() -> Result<()> {
    let git_dir = find_git_dir()?;
    let hooks_dir = git_dir.join("hooks");

    remove_hook(&hooks_dir.join("post-commit"))?;
    remove_hook(&hooks_dir.join("post-checkout"))?;

    println!("  \u{2713} Myceliums git hooks removed.");
    Ok(())
}
