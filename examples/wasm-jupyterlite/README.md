# A pixi example to build an emscripten / WASM environment

Read more in [our blog post](https://prefix.dev/blog/pixi_wasm)

Did you know that pixi can handle conda packages that are compiled to WebAssembly? This example shows how to build a simple environment to start & deploy a JupyterLite project with some WASM packages installed.

To deploy to Github pages using `pixi`, you can use the following Github Actions workflow (adapted from the official Jupyterlite demo repo).

You can find a full example (and demo deployment) in [this GitHub repository](https://github.com/wolfv/pixi-wasm) and the [deployed site](https://wolfv.github.io/pixi-wasm/).

```yaml
name: Build and Deploy

on:
  push:
    branches:
      - main
  pull_request:
    branches:
      - '*'

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - name: Checkout
        uses: actions/checkout@v4
      - name: Setup pixi project
        uses: prefix-dev/setup-pixi@v0.8.1
      - name: Build dist
        run: pixi run build-dist
      - name: Upload artifact
        uses: actions/upload-pages-artifact@v3
        with:
          path: ./dist

  deploy:
    needs: build
    if: github.ref == 'refs/heads/main'
    permissions:
      pages: write
      id-token: write

    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}

    runs-on: ubuntu-latest
    steps:
      - name: Deploy to GitHub Pages
        id: deployment
        uses: actions/deploy-pages@v4
```
