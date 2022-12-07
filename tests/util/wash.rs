use super::{Noop, Surround, Tee};

use anyhow::{bail, Context};
use wasi_common::pipe::{ReadPipe, WritePipe};
use wasi_common::{I32Exit, WasiDir, WasiFile};
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::{stdio, WasiCtxBuilder};

pub async fn wash(
    dir: Box<dyn WasiDir>,
    stdin: impl AsRef<str>,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    let engine = Engine::default();
    let module = Module::from_binary(&engine, include_bytes!(env!("CARGO_BIN_FILE_WASH")))
        .context("failed to compile `wash`")?;
    let mut linker = Linker::new(&engine);
    wasmtime_wasi::add_to_linker(&mut linker, |s| s).context("failed to link WASI")?;

    let stdout = WritePipe::new_in_memory();
    let stderr = WritePipe::new_in_memory();
    let code = {
        const EXIT: &str = r#"exit
"#;
        let stdin = stdin.as_ref();
        let stdin: Box<dyn WasiFile> = if cfg!(feature = "interactive") {
            println!(
                r#"stdin:
{stdin}"#
            );
            Box::new(Surround {
                left: ReadPipe::from(stdin),
                inner: stdio::stdin(),
                right: ReadPipe::from(EXIT),
            })
        } else {
            Box::new(Surround {
                left: ReadPipe::from(""),
                inner: ReadPipe::from(stdin),
                right: ReadPipe::from(EXIT),
            })
        };
        let stdout: Box<dyn WasiFile> = if cfg!(feature = "interactive") {
            Box::new(Tee {
                inner: stdout.clone(),
                read: Noop,
                write: stdio::stdout(),
            })
        } else {
            Box::new(stdout.clone())
        };
        let stderr: Box<dyn WasiFile> = if cfg!(feature = "interactive") {
            Box::new(Tee {
                inner: stderr.clone(),
                read: Noop,
                write: stdio::stderr(),
            })
        } else {
            Box::new(stderr.clone())
        };

        let mut ctx = WasiCtxBuilder::new()
            .arg("main.wasm")
            .context("failed to set argv[0]")?
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr)
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
            .map_err(|e| e.downcast().map(|I32Exit(code)| code))
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
