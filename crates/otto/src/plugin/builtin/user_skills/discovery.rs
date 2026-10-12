//! Five-tier discovery for `SKILL.md` definitions. Same precedence as
//! sub-projects A, B, and C: project beats user, `.otto/` beats
//! `.claude/`. First-wins dedup by directory slug.
//!
//! The spec names four tiers; `<project>/.github/skills/` is a fifth,
//! added because it is where Copilot CLI puts skills and where repos
//! that predate otto already keep them. It sits
//! last among the project tiers and has no user-scope counterpart,
//! because Copilot CLI only defines it inside a repository.
//!
//! Unlike agents, a skill is a directory — `skills/<slug>/SKILL.md` —
//! because the format carries level-3 resources (`scripts/`,
//! `references/`, `assets/`) alongside the instructions.

use std::path::{Path, PathBuf};

use crate::plugin::builtin::user_skills::frontmatter::parse;
use crate::plugin::builtin::user_skills::spec::{SkillScope, SkillSpec};

/// Script extensions that count as "runnable" for the level-3 trust
/// gate on platforms without a meaningful executable bit.
const SCRIPT_EXTENSIONS: &[&str] = &["sh", "bash", "zsh", "py", "rb", "js", "ts", "pl", "ps1"];

/// Outcome of a discovery walk: the skills that loaded, plus one
/// human-readable line per skill that was skipped.
///
/// Warnings are both traced and returned. Tracing alone is enough for a
/// developer reading logs, but `/skills` needs a *count* to tell the user
/// their `SKILL.md` was ignored — silently listing 2 of 3 skills is the
/// kind of quiet failure that sends people hunting through source.
#[derive(Debug, Default)]
pub struct Discovered {
    pub skills: Vec<SkillSpec>,
    pub warnings: Vec<String>,
}

/// Discover skills across the four standard tiers. Malformed skills warn
/// and are skipped — one bad `SKILL.md` never aborts the walk.
pub fn discover(project_root: &Path, user_home: &Path) -> Discovered {
    let tiers = [
        (
            project_root.join(".otto").join("skills"),
            SkillScope::ProjectOtto,
        ),
        (
            project_root.join(".claude").join("skills"),
            SkillScope::ProjectClaude,
        ),
        (
            project_root.join(".github").join("skills"),
            SkillScope::ProjectGithub,
        ),
        (user_home.join(".otto").join("skills"), SkillScope::UserOtto),
        (
            user_home.join(".claude").join("skills"),
            SkillScope::UserClaude,
        ),
    ];

    let mut out = Discovered::default();
    let mut seen: std::collections::HashMap<String, SkillScope> = Default::default();

    for (dir, scope) in tiers {
        for skill_dir in skill_dirs(&dir) {
            let slug = match slug_from_dir(&skill_dir) {
                Ok(slug) => slug,
                Err(e) => {
                    tracing::warn!("skill {skill_dir:?} skipped: {e}");
                    out.warnings.push(e);
                    continue;
                }
            };
            // First tier to claim a slug wins; later copies are shadowed,
            // not errors. Reported so an author who edited the losing copy
            // finds out why nothing changed.
            if let Some(winner) = seen.get(&slug) {
                let w = format!(
                    "skill `{slug}` at {skill_dir:?} shadowed by the copy in {}",
                    winner.label()
                );
                tracing::warn!("{w}");
                out.warnings.push(w);
                continue;
            }
            match load_skill(&skill_dir, &slug, scope.clone()) {
                Ok(spec) => {
                    seen.insert(slug, scope.clone());
                    out.skills.push(spec);
                }
                Err(e) => {
                    let w = format!("skill {skill_dir:?} skipped: {e}");
                    tracing::warn!("{w}");
                    out.warnings.push(w);
                }
            }
        }
    }

    out
}

/// Immediate subdirectories of `dir` that contain a `SKILL.md`. Sorted
/// so discovery order is deterministic across filesystems.
fn skill_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("SKILL.md").is_file() {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// The directory name, validated as the skill's index key. Returns the
/// rejection reason so the caller can both trace it and surface a count.
fn slug_from_dir(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("skill {path:?} has no readable directory name"))?;
    if name.is_empty() {
        return Err(format!("skill {path:?} has an empty directory name"));
    }
    if name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        Ok(name.to_string())
    } else {
        Err(format!(
            "invalid slug `{name}` (must be lowercase-kebab-case)"
        ))
    }
}

fn load_skill(dir: &Path, slug: &str, scope: SkillScope) -> Result<SkillSpec, String> {
    let source = dir.join("SKILL.md");
    let raw = std::fs::read_to_string(&source).map_err(|e| e.to_string())?;
    let parsed = parse(&raw, slug)?;
    for w in &parsed.warnings {
        tracing::warn!("skill {source:?}: {w}");
    }
    Ok(SkillSpec {
        name: parsed.name,
        description: parsed.description,
        allowed_tools: parsed.allowed_tools,
        root: dir.to_path_buf(),
        source,
        scope,
        body: parsed.body,
        has_bundled_executables: has_bundled_executables(dir),
    })
}

/// Whether the skill directory carries anything the model could run.
/// Drives the level-3 project-trust prompt (Task 2). Cheap and
/// deliberately conservative — a false positive costs one prompt.
fn has_bundled_executables(dir: &Path) -> bool {
    if dir.join("scripts").is_dir() {
        return true;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| SCRIPT_EXTENSIONS.contains(&ext))
        {
            return true;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if entry
                .metadata()
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    const MINIMAL: &str = "---\ndescription: test skill\n---\nbody";

    fn write_skill(skills_dir: &Path, slug: &str, contents: &str) -> PathBuf {
        let dir = skills_dir.join(slug);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), contents).unwrap();
        dir
    }

    #[test]
    fn finds_skills_in_all_five_tiers() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        write_skill(&project.path().join(".otto/skills"), "a", MINIMAL);
        write_skill(&project.path().join(".claude/skills"), "b", MINIMAL);
        write_skill(&project.path().join(".github/skills"), "e", MINIMAL);
        write_skill(&home.path().join(".otto/skills"), "c", MINIMAL);
        write_skill(&home.path().join(".claude/skills"), "d", MINIMAL);

        let found = discover(project.path(), home.path()).skills;
        let mut names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["a", "b", "c", "d", "e"]);
    }

    /// `.github/` is the lowest project tier, so a repo that keeps the
    /// same skill in both `.claude/` and `.github/` gets the `.claude/`
    /// copy — the format otto's docs tell authors to write.
    #[test]
    fn precedence_project_claude_beats_project_github() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        write_skill(
            &project.path().join(".claude/skills"),
            "dup",
            "---\ndescription: project claude\n---\nwinner",
        );
        write_skill(
            &project.path().join(".github/skills"),
            "dup",
            "---\ndescription: project github\n---\nloser",
        );

        let found = discover(project.path(), home.path()).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "project claude");
    }

    /// But `.github/` is still a *project* tier, so it outranks every
    /// user tier — a repo's own skill wins over a same-named personal one.
    #[test]
    fn precedence_project_github_beats_user_otto() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        write_skill(
            &project.path().join(".github/skills"),
            "dup",
            "---\ndescription: project github\n---\nwinner",
        );
        write_skill(
            &home.path().join(".otto/skills"),
            "dup",
            "---\ndescription: user otto\n---\nloser",
        );

        let found = discover(project.path(), home.path()).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "project github");
    }

    #[test]
    fn github_tier_is_labelled_distinctly() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        write_skill(&project.path().join(".github/skills"), "a", MINIMAL);

        let found = discover(project.path(), home.path()).skills;
        assert_eq!(found[0].scope.label(), "project/.github");
    }

    #[test]
    fn precedence_project_otto_beats_every_other_tier() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        write_skill(
            &project.path().join(".otto/skills"),
            "dup",
            "---\ndescription: project otto\n---\nwinner",
        );
        write_skill(
            &project.path().join(".claude/skills"),
            "dup",
            "---\ndescription: project claude\n---\nloser",
        );
        write_skill(
            &home.path().join(".claude/skills"),
            "dup",
            "---\ndescription: user claude\n---\nloser",
        );

        let found = discover(project.path(), home.path()).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "project otto");
        assert_eq!(found[0].scope, SkillScope::ProjectOtto);
    }

    #[test]
    fn precedence_project_claude_beats_user_otto() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        write_skill(
            &project.path().join(".claude/skills"),
            "dup",
            "---\ndescription: project claude\n---\nwinner",
        );
        write_skill(
            &home.path().join(".otto/skills"),
            "dup",
            "---\ndescription: user otto\n---\nloser",
        );

        let found = discover(project.path(), home.path()).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "project claude");
    }

    #[test]
    fn directory_without_skill_md_is_skipped() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        let stray = project.path().join(".otto/skills/not-a-skill");
        fs::create_dir_all(&stray).unwrap();
        fs::write(stray.join("README.md"), "nope").unwrap();

        assert!(discover(project.path(), home.path()).skills.is_empty());
    }

    #[test]
    fn malformed_skill_is_skipped_without_aborting_the_walk() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        write_skill(
            &project.path().join(".otto/skills"),
            "aaa-broken",
            "no frontmatter",
        );
        write_skill(&project.path().join(".otto/skills"), "zzz-good", MINIMAL);

        let found = discover(project.path(), home.path()).skills;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "zzz-good");
    }

    #[test]
    fn invalid_slug_is_skipped() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        write_skill(&project.path().join(".otto/skills"), "Not_Valid", MINIMAL);
        assert!(discover(project.path(), home.path()).skills.is_empty());
    }

    #[test]
    fn records_skill_root_and_source() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        let dir = write_skill(&project.path().join(".otto/skills"), "a", MINIMAL);

        let found = discover(project.path(), home.path()).skills;
        assert_eq!(found[0].root, dir);
        assert_eq!(found[0].source, dir.join("SKILL.md"));
    }

    #[test]
    fn detects_bundled_scripts_directory() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        let dir = write_skill(&project.path().join(".otto/skills"), "a", MINIMAL);
        fs::create_dir_all(dir.join("scripts")).unwrap();

        let found = discover(project.path(), home.path()).skills;
        assert!(found[0].has_bundled_executables);
    }

    #[test]
    fn detects_bundled_script_by_extension() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        let dir = write_skill(&project.path().join(".otto/skills"), "a", MINIMAL);
        fs::write(dir.join("setup.py"), "print('hi')").unwrap();

        let found = discover(project.path(), home.path()).skills;
        assert!(found[0].has_bundled_executables);
    }

    #[test]
    fn plain_skill_has_no_bundled_executables() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        let dir = write_skill(&project.path().join(".otto/skills"), "a", MINIMAL);
        fs::write(dir.join("reference.md"), "notes").unwrap();

        let found = discover(project.path(), home.path()).skills;
        assert!(!found[0].has_bundled_executables);
    }

    #[test]
    fn missing_directories_are_not_an_error() {
        let project = tempdir().unwrap();
        let home = tempdir().unwrap();
        assert!(discover(project.path(), home.path()).skills.is_empty());
    }
}
