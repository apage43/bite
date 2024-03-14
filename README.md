with [rust toolchain](https://rustup.rs/) installed:

```bash
cargo install --git https://github.com/apage43/bite
```

---

`bite instance-name` searches for `Host instance-name` in `.ssh/config` and updates the `HostName` line to point at the private IP of an instance identified with a `# bite: i-1a2b...` comment above the block, optionally (`-b/--boot`) starting it if it is stopped, then waits until SSH is up, so that you can `bite -b myinstance && ssh myinstance`
