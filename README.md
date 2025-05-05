# Bitely

[![Release v0.1.0](https://img.shields.io/badge/Release-v0.1.0-brightgreen)](https://github.com/Dangujba/bite-registry/releases/tag/v0.1.0) [![License](https://img.shields.io/badge/License-MIT-blue)](https://github.com/Dangujba/bite-registry/blob/main/LICENSE)

**Bitely** is the official package manager for the EasyBite programming language. It simplifies the process of publishing, discovering, installing, and removing EasyBite libraries and tools via a central registry.

---

## 🚀 Features

- **Publish**: Upload your EasyBite packages with metadata (name, version, description).
- **Install**: Fetch packages (and dependencies) by name or version, with optional offline and tree views.
- **Uninstall**: Remove installed packages safely, with a `--force` option.
- **Info & Search**: Inspect package metadata and search the registry by keyword.
- **Config**: View and override registry and cache settings.
- **Lightweight**: Single binary, no external dependencies beyond EasyBite v0.3.0+.

---

## 📥 Installation

### Windows
Bitely is included automatically when installing EasyBite via the `.msi` installer.

### macOS / Linux
1. Download the latest binary from the [Releases page](https://github.com/Dangujba/bitely-registry/releases).
2. Extract and move to a directory in your `PATH`:
   ```bash
   tar xzf bitely-v0.1.0.tar.gz
   sudo mv bitely /usr/local/bin/
   ```
3. Verify:
   ```bash
   bitely --version   # should output v0.1.0
   ```

---

## 💡 Usage

```bash
Bitely - EasyBite Package Manager

Usage: bitely [COMMAND] [OPTIONS]

Commands:
  publish    Publish a package to the Bitely registry
  install    Install a package or multiple packages
  uninstall  Remove an installed package
  info       Show information about a package
  search     List all available packages
  config     Display current Bitely config
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### Common Commands

- **Publish**
  ```bash
  bitely publish --name my_lib --path ./src --version 1.0.0 --description "My EasyBite library"
  ```

- **Install**
  ```bash
  bitely install my_lib@1.0.0         # install specific version
  bitely install my_lib               # install latest
  bitely install --file deps.txt      # batch install from file
  bitely install my_lib --tree        # show dependency tree
  bitely install my_lib --offline     # use cache only
  bitely install my_lib --download    # download without installing
  ```

- **Uninstall**
  ```bash
  bitely uninstall my_lib             # remove package
  bitely uninstall my_lib --force     # force removal
  ```

- **Info**
  ```bash
  bitely info my_lib                  # show metadata
  bitely info my_lib@1.0.0            # specific version
  ```

- **Search**
  ```bash
  bitely search parser                # find packages matching "parser"
  bitely search --offline             # search local cache
  ```

- **Config**
  ```bash
  bitely config                       # view current settings
  ```

---

## ⚙️ Configuration & Environment

Bitely supports the following environment variables and flags:

- `BITE_REGISTRY`  
  Override the default registry URL (default: `https://github.com/Dangujba/bite-registry`).

- `BITE_MODULES`  
  Custom local path for bite modules.

- `--registry <url>`  
  Override registry for a single command.

---

## 🤝 Contributing

Contributions are welcome! Please fork the repo, create a branch for your feature or bugfix, and submit a pull request.

1. Fork the repository
2. Create a new branch: `git checkout -b feature/awesome-feature`
3. Make your changes and commit: `git commit -m "Add awesome feature"
4. Push to your branch: `git push origin feature/awesome-feature`
5. Open a Pull Request

Please read [`CONTRIBUTING.md`](https://github.com/Dangujba/bite-registry/blob/main/CONTRIBUTING.md) for more details.

---

## 📄 License

This project is licensed under the MIT License. See the [LICENSE](https://github.com/Dangujba/bite-registry/blob/main/LICENSE) file for details.

