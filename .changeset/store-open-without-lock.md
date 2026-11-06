---
tm-cli: patch
---

Opening a Workspace's store no longer takes the write lock to check the Workspace id it already holds, so a read such as `tm list` no longer waits on, or fails behind, another process's write.
