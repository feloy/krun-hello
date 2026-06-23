use std::ffi::CString;
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;

use libc::{fork, tcgetattr, tcsetattr, waitpid, STDIN_FILENO, TCSAFLUSH};
use krun_sys::{
    krun_add_virtio_console_default, krun_add_virtiofs, krun_create_ctx, krun_set_exec,
    krun_set_log_level, krun_set_vm_config, krun_set_workdir, krun_start_enter,
};

// dlopen is in libSystem on macOS — always linked, no extra dependency needed.
extern "C" {
    fn dlopen(filename: *const c_char, flag: i32) -> *mut std::ffi::c_void;
}
const RTLD_NOW: i32 = 0x2;
const RTLD_GLOBAL: i32 = 0x8;

// libkrun loads libkrunfw via dlopen() at runtime using just the filename, so
// dylibbundler won't see it as a static dependency and DYLD_LIBRARY_PATH won't
// help under hardened runtime. We pre-load it as RTLD_GLOBAL from the bundled
// libs/ directory so that libkrun's own dlopen() finds it already in memory.
// During development (no libs/ dir next to the binary) this is a no-op.
fn preload_krunfw() {
    let Ok(exe) = std::env::current_exe() else { return };
    let libs = exe.parent().unwrap_or(Path::new(".")).join("libs");
    let Ok(entries) = std::fs::read_dir(&libs) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("libkrunfw") && s.ends_with(".dylib") {
            if let Some(p) = entry.path().to_str().and_then(|s| CString::new(s).ok()) {
                unsafe { dlopen(p.as_ptr(), RTLD_NOW | RTLD_GLOBAL); }
            }
            break;
        }
    }
}

fn debug_enabled() -> bool {
    std::env::var("KRUN_DEBUG").is_ok()
}

macro_rules! dbg_log {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!("[krun-hello] {}", format!($($arg)*));
        }
    };
}

fn check(call: &str, ret: i32) {
    if ret < 0 {
        eprintln!("libkrun: {} failed (code {})", call, ret);
        std::process::exit(1);
    }
    dbg_log!("{} -> {}", call, ret);
}

fn main() {
    let rootfs = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: krun-hello <rootfs-path>");
        eprintln!();
        eprintln!("  <rootfs-path>  Path to a Linux root filesystem directory.");
        eprintln!();
        eprintln!("Quick setup with Docker + Alpine:");
        eprintln!("  docker create --name tmp alpine sh");
        eprintln!("  mkdir -p /tmp/rootfs");
        eprintln!("  docker export tmp | tar -C /tmp/rootfs -x");
        eprintln!("  docker rm tmp");
        eprintln!("  cargo run -- /tmp/rootfs");
        std::process::exit(1);
    });

    if debug_enabled() {
        // 5 = trace — lets libkrun emit its own internal logs to stderr.
        unsafe { krun_set_log_level(5) };
    }

    preload_krunfw();

    println!("Booting libkrun VM (rootfs: {})...", rootfs);

    // Save terminal state before the VM's virtio console puts it in raw mode.
    // We restore it in the parent after the child's _exit() bypasses atexit.
    let mut saved_term: libc::termios = unsafe { std::mem::zeroed() };
    let term_saved = unsafe { tcgetattr(STDIN_FILENO, &mut saved_term) } == 0;
    dbg_log!("tcgetattr(stdin) -> term_saved={}", term_saved);
    dbg_log!(
        "isatty: stdin={} stdout={} stderr={}",
        unsafe { libc::isatty(0) },
        unsafe { libc::isatty(1) },
        unsafe { libc::isatty(2) },
    );

    let ctx_id = unsafe {
        let ctx_id = krun_create_ctx();
        assert!(ctx_id >= 0, "krun_create_ctx failed: {}", ctx_id);
        let ctx_id = ctx_id as u32;
        dbg_log!("krun_create_ctx -> ctx_id={}", ctx_id);

        // 1 vCPU, 512 MiB RAM.
        check("krun_set_vm_config", krun_set_vm_config(ctx_id, 1, 512));

        // Expose the host rootfs directory to the VM via virtio-fs.
        // KRUN_FS_ROOT_TAG ("/dev/root") tells the kernel to mount this as /.
        let tag = CString::new("/dev/root").unwrap();
        let path = CString::new(rootfs.as_str()).unwrap();
        check(
            "krun_add_virtiofs",
            krun_add_virtiofs(ctx_id, tag.as_ptr(), path.as_ptr()),
        );

        // Wire the VM's console to our own stdin/stdout/stderr.
        // krun_add_virtio_console_default calls tcsetattr on the stdin fd to
        // put the terminal into raw mode; this fails with EINVAL if stdin is
        // not a TTY (e.g. CI, pipes). Fall back to /dev/null in that case —
        // the VM's output still reaches stdout via fd 1.
        let console_stdin = if libc::isatty(libc::STDIN_FILENO) == 1 {
            dbg_log!("stdin is a TTY, using fd 0 for console");
            libc::STDIN_FILENO
        } else {
            let devnull = CString::new("/dev/null").unwrap();
            let fd = libc::open(devnull.as_ptr(), libc::O_RDONLY);
            dbg_log!("stdin is not a TTY, opened /dev/null as fd {}", fd);
            fd
        };
        check(
            "krun_add_virtio_console_default",
            krun_add_virtio_console_default(ctx_id, console_stdin, 1, 2),
        );

        // Start in /.
        let workdir = CString::new("/").unwrap();
        check("krun_set_workdir", krun_set_workdir(ctx_id, workdir.as_ptr()));

        // Run: echo "Hello from libkrun VM!"
        let exec = CString::new("/bin/echo").unwrap();
        let arg0 = CString::new("echo").unwrap();
        let arg1 = CString::new("Hello from libkrun VM!").unwrap();
        let argv: &[*const c_char] = &[arg0.as_ptr(), arg1.as_ptr(), ptr::null()];

        let env_path = CString::new("PATH=/bin:/usr/bin").unwrap();
        let envp: &[*const c_char] = &[env_path.as_ptr(), ptr::null()];

        check(
            "krun_set_exec",
            krun_set_exec(ctx_id, exec.as_ptr(), argv.as_ptr(), envp.as_ptr()),
        );

        ctx_id
    };

    // Fork so the parent can restore the terminal after krun_start_enter's
    // internal _exit() tears down the child without running atexit handlers.
    let pid = unsafe { fork() };
    if pid < 0 {
        eprintln!("fork failed");
        std::process::exit(1);
    }
    dbg_log!("fork -> pid={}", pid);

    if pid == 0 {
        dbg_log!("child: calling krun_start_enter(ctx_id={})", ctx_id);
        // Child: boot the VM — krun_start_enter calls _exit() internally.
        let ret = unsafe { krun_start_enter(ctx_id) };
        eprintln!("VM exited with code: {}", ret);
        unsafe { libc::_exit(ret) };
    }

    // Parent: wait for the child, then restore the terminal.
    let mut wstatus: libc::c_int = 0;
    unsafe { waitpid(pid, &mut wstatus, 0) };
    dbg_log!(
        "waitpid: exited={} exit_code={} signaled={} signal={}",
        libc::WIFEXITED(wstatus),
        if libc::WIFEXITED(wstatus) { libc::WEXITSTATUS(wstatus) } else { -1 },
        libc::WIFSIGNALED(wstatus),
        if libc::WIFSIGNALED(wstatus) { libc::WTERMSIG(wstatus) } else { -1 },
    );
    unsafe {
        if term_saved {
            tcsetattr(STDIN_FILENO, TCSAFLUSH, &saved_term);
        }
    }
}
