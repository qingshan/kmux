# Installation and upgrades

## Requirements

- A jailbroken Kindle Oasis 10th generation (`kindlehf`) with KPM.
- A host running `kmux-proxy`, tmux and/or Herdr.
- SSH access from the proxy host to each configured machine.

The Kindle daemon must not be natively linked: `liblipc` exists on the device.
Only `kmux-proxy` is a native host binary.

## Build a package

Install the cross-toolchain once, then build a versioned package:

```sh
just toolchain
just package 0.5.32
```

The artifact is written to `dist/kmux_<version>_kindlehf.kpkg`. Build and
deploy a matching native proxy with:

```sh
just proxy
```

KPM only reinstalls when the version changes. Bump the package version after
daemon, WAF, or script changes; the build updates manifest and WAF cache-busting
URLs together.

## Sideload with KPM

Copy the package to the Kindle’s USB storage, then install it using KPM. For a
developer SSH sideload, the repository’s package layout can be refreshed with:

```sh
scp dist/kmux_<version>_kindlehf.kpkg kindle:/mnt/us/tmp/
ssh kindle 'tar xzf /mnt/us/tmp/kmux_<version>_kindlehf.kpkg \
  -C /mnt/us/kmc/kpm/packages/kmux --strip-components=1'
ssh kindle 'cd /mnt/us/kmc/kpm/packages/kmux && sh ./install.sh'
```

The installer refreshes the daemon, WAF, scriptlet, and startup registration.
It preserves `/mnt/us/kmux/var/`, including proxy settings and logs. Reopen
kmux after installation so Mesquite loads the new WAF assets.

## Configure the proxy

Create `/etc/kmux-proxy.json` on the proxy host. The minimum useful shape is:

```json
{
  "token": "shared-secret",
  "defaultHost": "devbox",
  "hosts": [
    { "id": "devbox", "session": "main" },
    { "id": "work", "backend": "herdr", "herdrSession": "coding" }
  ]
}
```

Run the proxy as the user whose SSH configuration and keys can reach those
machines. See the [proxy guide](../crates/kmux-proxy/README.md) for `local:<user>`,
named tmux sockets, Herdr startup, and API details.

## Rollback

Keep the previous `.kpkg`, proxy binary, and configuration until the new pair
has been tested. Reinstall the previous package version and restart the proxy
if the upgrade cannot connect. User configuration under `/mnt/us/kmux/var/`
is intentionally retained across upgrades.
