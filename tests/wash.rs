mod util;

use std::fs::File;

use anyhow::Context;
use tempfile::tempdir;
use tokio::test;
use wasmtime_vfs_ledger::Ledger;

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
    let dir = File::open(&tmp)
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
        ("file", Some(b"file".to_vec())),
        ("dir", None),
        ("dir/a.file", Some(b"file".to_vec())),
        ("dir/b.dir", None),
    ];

    // Construct the tmpfs tree.
    let mut builder = wasmtime_vfs_tmpfs::Builder::from(Ledger::new());
    for (path, data) in tree {
        builder = builder.add(path, data).await.unwrap();
    }
    let dir = builder.build();

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
