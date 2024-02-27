with [rust toolchain](https://rustup.rs/) installed:

```bash
cargo install --git https://github.com/apage43/bite
```

---

`bite instance-name ...` finds ec2 instance with Name tag `instance_name`, starts it if it isn't running, waits for ssh to come up, then passes rest of args through to `ssh`:

```
bite instance-name -A
```
