---
part: pixi
title: Authenticate a private pypi registry
description: Authenticate pixi to access private pypi registries.
---


`pixi` allows you to access private registries securely by authenticating with credentials stored in a `.netrc` file.

The `.netrc` file can be stored in your home directory (`$HOME/.netrc` for Unix-like systems) or in the user profile directory on Windows (`%HOME%\_netrc`). You can also set up a different location for it using the `NETRC` variable (`export NETRC=/my/custom/location/.netrc`).

In the `.netrc` file, you store authentication details like this:

```sh
Copy code
machine registry-name
login admin
password admin
```
For more details, you can access the [.netrc docs](https://www.ibm.com/docs/en/aix/7.2?topic=formats-netrc-file-format-tcpip).
