# krun-hello

A minimal Rust app that boots a real Linux virtual machine and prints "Hello from libkrun VM!" — using [libkrun](https://github.com/libkrun/libkrun).

## What it does

When you run `krun-hello`, it:

1. Spins up a lightweight Linux microVM on your Mac using Apple's [Hypervisor.framework](https://developer.apple.com/documentation/hypervisor) — no Docker, no QEMU, no root required.
2. Mounts a Linux root filesystem you provide into that VM via [virtio-fs](https://virtio-fs.gitlab.io/) (a high-performance host–guest filesystem share).
3. Executes `/bin/echo "Hello from libkrun VM!"` inside the VM.
4. Prints the output to your terminal and exits.

The whole boot-to-output cycle takes under a second. The VM is fully isolated: it runs its own Linux kernel with its own process namespace, but shares no persistent state with your host.

## How it uses libkrun

[libkrun](https://github.com/libkrun/libkrun) is a library that turns the virtual machine setup dance — kernel, memory, vCPUs, virtio devices — into a handful of function calls. Under the hood it uses Apple's Hypervisor.framework on macOS (the same primitive that powers Apple's own virtualization stack), so it requires no kernel extensions and no elevated privileges.

The Rust crate [`krun-sys`](https://crates.io/crates/krun-sys) provides generated FFI bindings to libkrun's C API. This app calls the following functions in order:

| Call | What it does |
|------|-------------|
| `krun_create_ctx()` | Allocates a new VM context; returns an integer context ID |
| `krun_set_vm_config(ctx, vcpus, ram_mib)` | Configures the VM with 1 vCPU and 512 MiB of RAM |
| `krun_add_virtiofs(ctx, "/dev/root", path)` | Shares the host rootfs directory into the VM; the tag `"/dev/root"` (`KRUN_FS_ROOT_TAG`) tells the bundled kernel to mount it as `/` |
| `krun_add_virtio_console_default(ctx, 0, 1, 2)` | Wires the VM's console to the host's stdin/stdout/stderr |
| `krun_set_workdir(ctx, "/")` | Sets the working directory inside the VM |
| `krun_set_exec(ctx, "/bin/echo", argv, envp)` | Sets the binary and arguments to run inside the VM |
| `krun_start_enter(ctx)` | Boots the VM — this call transfers control and never returns on success |

libkrun bundles its own Linux kernel via [libkrunfw](https://github.com/libkrun/homebrew-krun), so you do not need to supply or configure a kernel yourself.

## Prerequisites

### 1. Rust toolchain

Install via [rustup](https://rustup.rs/):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. libkrun + libkrunfw

Use the official tap — the main Homebrew tap does not carry libkrun:

```sh
brew tap libkrun/krun
brew install libkrun/krun/libkrun
```

This installs:
- `libkrun.dylib` — the VM library itself, plus its C headers (needed by `krun-sys` at build time)
- `libkrunfw.dylib` — the bundled Linux kernel, pulled in automatically as a runtime dependency

### 3. LLVM (for bindgen)

`krun-sys` generates its FFI bindings at build time using [bindgen](https://github.com/rust-lang/rust-bindgen), which requires `libclang`:

```sh
brew install llvm
```

Add the following to your `~/.zshrc` so the build and runtime linker can find both LLVM and the krun libraries:

```sh
export DYLD_LIBRARY_PATH="$(brew --prefix)/lib:$(brew --prefix llvm)/lib:$DYLD_LIBRARY_PATH"
```

Then reload your shell:

```sh
source ~/.zshrc
```

### 4. A Linux root filesystem

The VM needs a Linux rootfs directory to boot into. The quickest way to get one is to export a Docker container:

```sh
docker create --name tmp alpine sh
mkdir -p /tmp/rootfs
docker export tmp | tar -C /tmp/rootfs -x
docker rm tmp
```

This gives you a minimal Alpine Linux filesystem at `/tmp/rootfs`.

## Build and run

On macOS, processes that use `Hypervisor.framework` must be signed with the `com.apple.security.hypervisor` entitlement. The provided `run.sh` script handles this automatically:

```sh
chmod +x run.sh
./run.sh /tmp/rootfs
```

It builds the binary, signs it with `entitlements.plist`, then runs it. You cannot use `cargo run` directly — every `cargo run` rebuilds the binary, which clears the code signature.

Expected output:

```
Booting libkrun VM (rootfs: /tmp/rootfs)...
Hello from libkrun VM!
```

## Project structure

```
src/main.rs        — VM setup and boot logic (all libkrun calls)
Cargo.toml         — single dependency: krun-sys
entitlements.plist — Hypervisor.framework entitlement for macOS signing
run.sh             — build, sign, and run in one step
```

## Troubleshooting

**`krun-sys` version mismatch** — if the crates.io release doesn't match your installed libkrun version, pin it to the repo directly:

```toml
[dependencies]
krun-sys = { git = "https://github.com/libkrun/libkrun" }
```

**`libkrunfw` not found at runtime** — libkrun loads the kernel via `dlopen` at runtime, so it needs `DYLD_LIBRARY_PATH` to include the Homebrew lib directory (see Prerequisites above).

**`pkg-config` can't find libkrun** — make sure Homebrew's prefix is on your `PKG_CONFIG_PATH`:

```sh
export PKG_CONFIG_PATH="$(brew --prefix)/lib/pkgconfig:$PKG_CONFIG_PATH"
```
