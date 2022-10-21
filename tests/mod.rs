use std::fs::File;
use std::io::Read;
use std::sync::Arc;

use anyhow::{bail, Context};
use tempfile::tempdir;
use tokio::test;
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasi_common::WasiDir;
use wasmtime::{Engine, Linker, Module, Store, Trap};
use wasmtime_vfs_ledger::Ledger;
use wasmtime_wasi::WasiCtxBuilder;

fn wash<I: Read + Sync + Send + 'static>(
    dir: Box<dyn WasiDir>,
    stdin: impl Into<ReadPipe<I>>,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let engine = Engine::default();
    let module = Module::from_binary(&engine, include_bytes!(env!("CARGO_BIN_FILE_WASH")))
        .context("failed to compile `wash`")?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::add_to_linker(&mut linker, |s| s).context("failed to link WASI")?;

    let stdout = WritePipe::new_in_memory();
    let stderr = WritePipe::new_in_memory();
    let code = {
        let mut ctx = WasiCtxBuilder::new()
            .arg("main.wasm")
            .context("failed to set argv[0]")?
            .stdin(Box::new(stdin.into()))
            .stdout(Box::new(stdout.clone()))
            .stderr(Box::new(stderr.clone()))
            .build();
        ctx.push_preopened_dir(dir, "/")
            .context("failed to push directory")?;

        let mut store = Store::new(&engine, ctx);
        linker
            .module(&mut store, "", &module)
            .context("failed to link Wasm module")?;
        linker
            .get_default(&mut store, "")
            .context("failed to get default function")?
            .typed::<(), (), _>(&store)
            .context("failed to assert default function type")?
            .call(&mut store, ())
            .as_ref()
            .map_err(Trap::i32_exit_status)
            .expect_err("`wash` did not set an exit code, did you forget to execute `exit`?")
            .context("failed to execute default function")?
    };
    let stdout = stdout
        .try_into_inner()
        .expect("failed to acquire stdout")
        .into_inner();
    let stderr = stderr
        .try_into_inner()
        .expect("failed to acquire stderr")
        .into_inner();
    if code != 0 {
        bail!(
            r#"`wash` exited with code `{code}`
stdout: {}
stderr: {}
"#,
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr),
        )
    }
    Ok((stdout, stderr))
}

#[test]
async fn wasi_sync_dir() -> anyhow::Result<()> {
    const CMD: &str = r#"ls /
echo 'test' > test
cat test
exit
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
    let (out, err) = wash(dir, CMD).context("failed to execute `wash`")?;
    let out = String::from_utf8_lossy(&out);
    let err = String::from_utf8_lossy(&err);
    assert_eq!(out, OUT, "{err}");
    Ok(())
}

#[test]
async fn tmpfs() -> anyhow::Result<()> {
    const CMD: &str = r#"ls /
echo 'test' > test
ls /
cat test
ls /dir
exit
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
    for (path, data) in tree.into_iter() {
        builder = builder.add(path, data).await.unwrap();
    }
    let dir = builder.build();

    // Run the script and test the output.
    let (out, err) = wash(dir, CMD).context("failed to execute `wash`")?;
    let out = String::from_utf8_lossy(&out);
    let err = String::from_utf8_lossy(&err);
    assert_eq!(out, OUT, "{err}");
    Ok(())
}
