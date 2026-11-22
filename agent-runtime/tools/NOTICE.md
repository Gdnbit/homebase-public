# Linux sandbox executable

`bwrap` is the amd64 executable extracted from Ubuntu noble's security-updated
`bubblewrap_0.9.0-1ubuntu0.3_amd64.deb`. It is invoked inside WSL Ubuntu.
No global package installation or sudo setup is performed.

- Official package and corresponding source/security patches: https://archive.ubuntu.com/ubuntu/pool/main/b/bubblewrap/
- Upstream: https://github.com/containers/bubblewrap
- Package SHA-256: `2461f1beee9cb04c8942739fe1a2b37e7b7c2a3d518f0779dc75f9245baa3094`
- Executable SHA-256: `e318903862396f96de3df57264e0158682b952fd3fb53ac23d876413e7b30f71`
- Full copyright/license: `bubblewrap-copyright`.

Matching source and Ubuntu packaging patches are in the official archive under
version `0.9.0-1ubuntu0.3`. The runtime checks the executable hash on every invocation;
a mismatch does not fall back to a host shell.

This source checkout excludes the executable. Obtain it from the pinned Ubuntu
package and set `HOMEBASE_BWRAP_SOURCE` to the absolute path of the extracted
executable before Windows packaging or its smoke test. Packaging checks the
pinned SHA-256 before including it.
