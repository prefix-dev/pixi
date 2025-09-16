# Pixi Extensions

`Pixi` allows you to extend its functionality with various extensions.
When executing e.g. `pixi diff`, Pixi will search for the executable `pixi-diff` in your PATH and in your pixi global directories.
Then it will execute it by passing any additional arguments to it.

## How Extensions Work

Pixi extensions are standalone executables that follow a simple naming convention: they must be named `pixi-{command}` where `{command}` is the name of the subcommand you want to add. When you run `pixi {command}`, Pixi will automatically discover and execute the corresponding `pixi-{command}` executable.

For example:
- `pixi diff` → looks for `pixi-diff` executable
- `pixi pack` → looks for `pixi-pack` executable  
- `pixi deploy` → looks for `pixi-deploy` executable

## Extension Discovery

Pixi discovers extensions by searching for `pixi-*` executables in the following locations, in order:

### 1. PATH Environment Variable
Pixi searches all directories in your `PATH` environment variable for executables with the `pixi-` prefix.

### 2. `pixi global` Directories  
Pixi also searches in directories managed by `pixi global`, which allows for organized extension management without cluttering your system PATH.

When you run `pixi --list`, all discovered extensions are automatically listed alongside all built-in commands, making the commands easily discoverable.

## Installing Extensions

### Using `pixi global` (Recommended)

The easiest way to install Pixi extensions is using `pixi global install`:

```bash
# Install a single extension
pixi global install pixi-pack

# Install multiple extensions at once
pixi global install pixi-pack pixi-diff
```

This approach has several advantages:
- **Isolated environments**: Each extension gets its own environment, preventing dependency conflicts
- **Automatic discovery**: Extensions are automatically found by Pixi without modifying PATH
- **Easy management**: Use `pixi global list` and `pixi global remove` to manage extensions
- **Consistent experience**: Extensions appear in `pixi --list` with all the built-in commands, just like how `Cargo` handles it

### Manual Installation

You can also install extensions manually by placing the executable in any directory in your PATH:

```bash
# Download or build the extension
curl -L https://github.com/user/pixi-myext/releases/download/v1.0.0/pixi-myext -o pixi-myext
chmod +x pixi-myext
mv pixi-myext ~/.local/bin/
```

## Contributing Extensions

### Creating an Extension

1. **Choose a descriptive name**: Your extension should be named `pixi-{command}` where `{command}` clearly describes its functionality.

2. **Create the executable**: Extensions can be written in any language (Rust, Python, shell scripts, etc.) as long as they produce an executable binary.

3. **Handle arguments**: Extensions receive all arguments passed after the command name.

### Example: Simple Python Extension

```python
#!/usr/bin/env python3
import sys

def main():
    name = sys.argv[1] if len(sys.argv) > 1 else "World"
    print(f"Hello, {name}!")

if __name__ == "__main__":
    main()
```

Save this as `pixi-hello`, make it executable (`chmod +x pixi-hello`), and place it in your PATH.

Usage: `pixi hello Alice` outputs `Hello, Alice!`

## Best Practices

- **Use standard argument parsing**: Libraries like `clap` (Rust) or `argparse` (Python) provide consistent behavior
- **Support `--help`**: Users expect this standard flag
- **Follow UNIX conventions**: Use exit code 0 for success, non-zero for errors
- **Work with Pixi environments**: Extensions should respect Pixi's environment management

## Command Suggestions

Pixi includes intelligent command suggestions powered by string similarity. If you mistype a command name, Pixi will suggest the closest match from both built-in commands and available extensions:

```bash
$ pixi pck
error: unrecognized subcommand 'pck`
tip: a similar subcommand exists: 'pack'
```

This works for both built-in commands and any extensions you have installed, making extension discovery seamless.

## Getting Help

- **List available extensions**: Run `pixi --list` to see all available extensions
- **Community**: Join our [Discord](https://discord.gg/kKV8ZxyzY4) for discussions and support

## See Also

- [Pixi Diff](pixi_diff.md) - Compare lock files and environments  
- [Pixi Inject](pixi_inject.md) - Inject dependencies into existing environments
- [Global Tools](../../global_tools/introduction.md) - Managing global tool installations
