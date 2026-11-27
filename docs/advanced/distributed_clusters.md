!!!warning "Only on Unix-like systems"
    The following approach only works on Unix-like systems (i.e. primarily Linux/BSD and in theory macOS).

For cluster systems with distributed filesystems, it is beneficial (though not mandatory) to keep pixi and its deployments away from the shared filesystem and work on local storage instead.
This way, we trade off I/O pressure with slight (but due to pixi's speed rather negligible) network traffic overhead.
This can be achieved by providing pixi via an sh function that installs it on-the-fly to a PIXI_HOME that is machine-local by creating/extending `~/.shrc` with the following:

```bash
# ~/.shrc
# Configuration of pixi deployment
# path to machine-local pixi installation
export PIXI_HOME=...
# path to desired (machine-local) cache. Ideally, this should be already defined by your admins and can be skipped in such a case. If not set it to a reasonable machine-local path as well.
export XDG_CACHE_HOME=...

# Self-deploying pixi entrypoint
pixi() {
    local entrypoint="$PIXI_HOME/bin/pixi"

    if [ ! -f $entrypoint ]
    then
        echo "First-time pixi setup..."
        (curl -fsSL https://pixi.sh/install.sh | PIXI_NO_PATH_UPDATE=true sh) > /dev/null
    fi

    # ensure that workspace envs are stored in the pixi cache
    command $entrypoint config set detached-environments true
    command $entrypoint "$@"
}
```

In addition, source `.shrc` in your preferred shell and ensure that `.shrc` is loaded by plain sh shells, by e.g. appending the following to `~/.bashrc`:

```bash
export ENV="$HOME/.shrc"
source $ENV
```

The result is that pixi will deploy itself whenever the function is called on a machine that does not have a pixi installation yet.
If the two environment variables `$PIXI_HOME` and `$XDG_CACHE_HOME` are setup accordingly, package cache, pixi itself, installations with `pixi global`, and any workspace environments will be machine-local, thus not causing pressure on the shared network filesystem.
