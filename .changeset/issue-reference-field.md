---
tm-cli: minor
---

A Scheme file's fields can be of type `issue_reference`, naming another Issue. `tm create`, `tm edit` and `tm move` take it by Key or id with `--set FIELD=KEY` and store the Issue's id; `tm show`, the change lines and `tm history` name it by Key, while `tm show --json` keeps the id. An Issue that does not exist, or the Issue itself, is refused. `tm export` writes it as the target's `tm:` source, and a record import writes it after every Issue in the file exists, as it does `parent`, so a record may name an Issue on a later line. Filtering by such a field is refused as not supported yet, though `has:` and `no:` work.
