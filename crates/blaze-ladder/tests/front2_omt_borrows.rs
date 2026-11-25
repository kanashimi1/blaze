//! OMT front-end borrows: the two false positives of the omt check, each rustc-accepted.
//!
//! - `OsString::to_str` chains (`entry.file_name().to_str().map(ToOwned::to_owned)`): a
//!   `let-else` binding's scope also held the following `if let`'s scrutinee temporaries
//!   (pre-2024: "the enclosing scope's"), and `close_scope` dropped the binding's variable
//!   before those temporaries, though they borrow it — a false "dropped while the borrow is
//!   live" / "goes out of scope while the borrow is live" (E0505/E0597). Fixed in
//!   blaze-flow-lower `close_scope` (the scope's own temporaries leave before its variables,
//!   exit_drops' per-scope rule).
//! - async fns with elided input lifetimes awaited and their error used (`run_validation(env)
//!   .await` then `error.message`): still OPEN — the coroutine's await output is related to
//!   the captured lifetimes, a spurious "a lifetime must outlive another and the signature
//!   does not say so". Kept here `#[ignore]`d as the minimal repro of that blocker.

#![feature(allocator_api)]

use blaze_ladder::crates::{DirShelf, Inputs, Toolchain, UserCrate};
use blaze_ladder::front2::borrows::BorrowVerdict;
use blaze_ladder::front2::project::{BodyRef, ProjectWorld};
use blaze_ladder::refusal::SourceFile;

const EDITION: &str = "2021";

fn check(main: &str, want: &[&str]) -> Vec<String> {
    let heap = blaze_own::testing::heap();
    let home = std::env::var("HOME").expect("HOME is set");
    let shelf = format!("{home}/.blaze/v2/toolchain");
    let tc = Toolchain::open(&mut DirShelf::new(shelf.clone())).unwrap_or_else(|e| panic!("the toolchain at {shelf}: {e}"));
    let files = [("/omt/src/main.rs".to_string(), main.to_string())];
    let edition = edition::PackageEdition::of_manifest(EDITION).expect("a known edition");
    let user = UserCrate { name: "omt", dir: "/omt/", root: "/omt/src/main.rs", files: &files, edition, panic: blaze_build::Panic::Unwind };
    let inputs = Inputs::new(&user, &tc).unwrap_or_else(|e| panic!("the crates: {e}"));
    let crates = inputs.crates();
    let names = inputs.resolve(&crates, blaze_own::testing::bump()).unwrap_or_else(|e| panic!("resolve_crates: {e}"));

    let sources = [SourceFile { path: "src/main.rs", text: main }];
    let frame = blaze_mem::Frame::new(blaze_mem::tag::Tag::Infer).expect("the command's frame");
    let mut world = ProjectWorld::new(&names, &frame, &sources, heap.alloc()).unwrap_or_else(|e| panic!("front2: {e}"));
    let bodies = world.bodies(heap.alloc());
    let name_of = |b: &BodyRef<'_>| -> String {
        match b {
            BodyRef::Free(k) => names.item_text(*k).and_then(|t| t.name).unwrap_or("?").to_string(),
            BodyRef::Member { name, .. } => name.to_string(),
        }
    };
    let mut out = Vec::new();
    for n in want {
        let b = bodies.iter().find(|(b, _)| name_of(b) == *n).map(|(b, _)| *b).unwrap_or_else(|| panic!("`{n}` listed"));
        let mut show = String::new();
        let (types, v) = world.check_borrows_shown(b, &mut show, heap.alloc());
        assert!(types.is_clean(), "{n}'s types: {types:?}");
        assert!(v.refused.is_none(), "{n} not checked: {v:?}");
        for d in &v.findings {
            out.push(format!("{n}: {} at {}..{}", d.code, d.start, d.end));
        }
    }
    out
}

/// The OsString pattern: a `let-else` binding of a `file_name().to_str()` chain, used by a
/// following `if let` whose scrutinee borrows it. The minimal core: a `let-else` binding, an
/// `if let` whose scrutinee borrows it, and nothing after — the binding's drop must come after
/// the scrutinee temporaries'.
#[test]
fn osstring_let_else_then_if_let_is_clean() {
    let src = r#"fn main() {}

pub fn find_named(dirs: &[std::path::PathBuf], id: &str) -> Option<std::path::PathBuf> {
    let mut available: Vec<u64> = Vec::new();
    for dir in dirs {
        let Ok(files) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in files.flatten() {
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            if let Some(rest) = name.strip_prefix(id).and_then(|r| r.strip_prefix('.')) {
                let _ = rest;
                return Some(entry.path());
            }
            if let Some(num) = name.split('.').next().and_then(|p| p.parse::<u64>().ok()) {
                if !available.contains(&num) {
                    available.push(num);
                }
            }
        }
    }
    let _ = available;
    None
}
"#;
    let found = check(src, &["find_named"]);
    assert!(found.is_empty(), "false positives: {found:?}");
}

/// The async-fn pattern (api_key.rs): an async fn with elided input lifetimes, awaited, its
/// error's message moved out. OPEN: the coroutine's await output is tied to the captured
/// lifetimes, a spurious outlives requirement ("the signature does not say so").
#[test]
#[ignore = "open checker bug: async-fn await output tied to captured lifetimes (spurious outlives)"]
fn async_fn_elided_await_then_error_use_is_clean() {
    let src = r#"fn main() {}

pub struct ValidationError {
    pub message: String,
}

async fn run_validation(
    h: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<(), ValidationError> {
    let _ = h("X");
    Ok(())
}

pub async fn login(
    env: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<String, String> {
    if let Err(error) = run_validation(env).await {
        return Err(error.message);
    }
    Ok(String::new())
}
"#;
    let found = check(src, &["login", "run_validation"]);
    assert!(found.is_empty(), "false positives: {found:?}");
}
