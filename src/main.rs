use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;

use krun_sys::{
    krun_add_virtio_console_default, krun_add_virtiofs, krun_create_ctx, krun_set_exec,
    krun_set_vm_config, krun_set_workdir, krun_start_enter,
};

fn check(call: &str, ret: i32) {
    if ret < 0 {
        eprintln!("libkrun: {} failed (code {})", call, ret);
        std::process::exit(1);
    }
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

    println!("Booting libkrun VM (rootfs: {})...", rootfs);

    unsafe {
        // Allocate a VM context; returns a non-negative ID on success.
        let ctx_id = krun_create_ctx();
        assert!(ctx_id >= 0, "krun_create_ctx failed: {}", ctx_id);
        let ctx_id = ctx_id as u32;

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
        check(
            "krun_add_virtio_console_default",
            krun_add_virtio_console_default(ctx_id, 0, 1, 2),
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

        // Boot the VM — transfers control and never returns on success.
        let ret = krun_start_enter(ctx_id);
        eprintln!("VM exited with code: {}", ret);
    }
}
