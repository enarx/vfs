mod util;

use anyhow::Context;
use tempfile::tempdir;
use tokio::test;
use wasmtime_vfs_ledger::Ledger;
use wasmtime_vfs_memory::Node;
use wasmtime_vfs_tmpfs::{Directory, File};

#[test]
#[cfg_attr(feature = "interactive", serial_test::serial)]
async fn wasi_sync_dir() -> anyhow::Result<()> {
    const CMD: &str = r#"ls /
echo 'test' > test
cat test
"#;

    const OUT: &str = r#"
test
"#;

    // Set up a temporary directory.
    let tmp = tempdir().context("failed to create a temporary directory")?;
    let dir = std::fs::File::open(&tmp)
        .map(wasmtime_wasi::sync::Dir::from_std_file)
        .map(wasmtime_wasi::sync::dir::Dir::from_cap_std)
        .map(Box::new)
        .with_context(|| format!("failed to open `{}`", tmp.path().display()))?;

    // Run the script and test the output.
    let (out, err) = util::wash(dir, CMD)
        .await
        .context("failed to execute `wash`")?;
    if cfg!(not(feature = "interactive")) {
        let out = String::from_utf8_lossy(&out);
        let err = String::from_utf8_lossy(&err);
        assert_eq!(out, OUT, "{err}");
    }
    Ok(())
}

#[test]
#[cfg_attr(feature = "interactive", serial_test::serial)]
async fn tmpfs() -> anyhow::Result<()> {
    const CMD: &str = r#"ls /
echo 'test' > test
ls /
cat test
ls /dir
"#;

    const OUT: &str = r#"dir file
dir file test
test
a.file b.dir
"#;

    let tree = [
        ("file", Some(b"file")),
        ("dir", None),
        ("dir/a.file", Some(b"file")),
        ("dir/b.dir", None),
    ];

    // Construct the tmpfs tree.
    let root = Directory::root(Ledger::new());
    for (path, data) in tree {
        root.attach(path, |p| match data {
            Some(data) => Ok(File::with_data(p, *data)),
            None => Ok(Directory::new(p)),
        })
        .await
        .unwrap();
    }
    let root = root.open_dir().await.unwrap();

    // Run the script and test the output.
    let (out, err) = util::wash(root, CMD)
        .await
        .context("failed to execute `wash`")?;
    if cfg!(not(feature = "interactive")) {
        let out = String::from_utf8_lossy(&out);
        let err = String::from_utf8_lossy(&err);
        assert_eq!(out, OUT, "{err}");
    }
    Ok(())
}
