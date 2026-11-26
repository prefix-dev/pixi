!!!warning "Only on Unix-like systems"
    The following approach only works on Unix-like systems (i.e. primarily Linux/BSD and in theory macOS).

For cluster systems with distributed filesystems, it is beneficial (though not mandatory) to keep Pixi and its deployments away from the shared filesystem and work on local storage instead.
This way, we trade off I/O pressure with slight (but due to Pixi's speed rather negligible) network traffic overhead.

## For users

For users without administrator permissions, this can be achieved by providing Pixi via an sh function that installs it on-the-fly to a PIXI_HOME that is machine-local by creating/extending `~/.shrc` with the following:

```bash
# ~/.shrc
# Configuration of Pixi deployment
# path to machine-local Pixi installation
export PIXI_HOME=...
# path to desired (machine-local) cache. Ideally, this should be already defined by your admins and can be skipped in such a case. If not set it to a reasonable machine-local path as well.
export XDG_CACHE_HOME=...
export PATH=$PIXI_HOME/bin:$PATH

# Self-deploying Pixi entrypoint
pixi() {
    local entrypoint="$PIXI_HOME/bin/pixi"

    if [ ! -f $entrypoint ]
    then
        echo "First-time Pixi setup..."
        (curl -fsSL https://pixi.sh/install.sh | PIXI_NO_PATH_UPDATE=true sh) > /dev/null
    fi

    # ensure that workspace envs are stored in the Pixi cache
    command $entrypoint config set detached-environments true
    command $entrypoint "$@"
}
```

In addition, source `.shrc` in your preferred shell and ensure that `.shrc` is loaded by plain sh shells, by e.g. appending the following to `~/.bashrc`:

```bash
export ENV="$HOME/.shrc"
source $ENV
```

The result is that Pixi will deploy itself whenever the function is called on a machine that does not have a Pixi installation yet.
If the two environment variables `$PIXI_HOME` and `$XDG_CACHE_HOME` are setup accordingly, package cache, Pixi itself, installations with `pixi global`, and any workspace environments will be machine-local, thus not causing pressure on the shared network filesystem.


## For administrators

Administrators can instead choose to orchestrate the installation of Pixi to each local machine via tools like Ansible.
In addition, one should (analogously to above user-specific settings) do the following.

Assuming that `$LOCALSCRATCH` is a path within a machine-local, non-distributed, filesystem.

1. One should instruct Pixi (as well as many other tools) to use a machine-local package cache, e.g. `export XDG_CACHE_HOME=$LOCALSCRATCH/$USER/cache`.
2. One should instruct Pixi to store its global data into a user specific but machine-local folder, e.g. `export PIXI_HOME=$LOCALSCRATCH/$USER/pixi`.
3. One should expose the global bin path of Pixi via `export PATH=$PIXI_HOME/bin:$PATH`
4. One should instruct Pixi to store deployed environments only machine-local (within `$PIXI_HOME`), using the following setting in `/etc/pixi/config.toml`:

```toml
detached-environments = true
```

Pixi/rattler will ensure that concurrent writes to all configured locations will be race condition free.

## Caveat: global installs become machine-local

An important caveat with both versions of above setup is that any installs via `pixi global` are machine-local.
In other words, users will have to repeat them on each machine before using the respective tools.
In contrast, any Pixi workspaces within the distributed filesystem will transparently self-deploy to the respective machine-local folders when used with `pixi run` or `pixi shell`.