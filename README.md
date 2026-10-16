# Angular Plus for Zed

A fork of [nathansbradshaw/zed-angular](https://github.com/nathansbradshaw/zed-angular) that adds:

- plain HTML completions (tags, attributes) in Angular templates next to the Angular ones, like VS Code
- Tailwind class completions in templates
- an optional completion proxy that keeps template completions fast while typing

## Overview

This extension integrates the Angular Language Service into Zed. It uses the same options that Angular applies during compilation. To ensure the most accurate information, enable the `strictTemplates` option in the `tsconfig.json` of the angular project as shown in below:

```json
"angularCompilerOptions": {
  "strictTemplates": true
}
```

## Requirements

The extension does not bundle or download a language server. It runs the copy of `@angular/language-server` installed in your project, using the Node binary managed by Zed. Install it alongside `typescript` as dev dependencies:

```sh
npm install --save-dev @angular/language-server typescript
```

By default the extension looks for the package at `node_modules/@angular/language-server`, relative to the root of the worktree you have open in Zed.

## Version Management

The extension depends on the `@angular/language-server` and `typescript` Node packages. It will use whatever versions of each that are available locally in your project.

The major version of `@angular/language-server` must match the Angular major version used by your project. TypeScript must be **5.0 or later**, and **6.0.3 is the latest supported version** — newer releases are untested and may fail to load. Mismatches typically surface as a `Failed to resolve 'typescript/lib/tsserverlibrary'` error in the language server logs.

If your project would otherwise pull in a newer TypeScript, pin it:

```json
{
  "devDependencies": {
    "typescript": "~6.0.3"
  }
}
```

Refer to [Angular Version Compatibility](https://angular.dev/reference/versions#unsupported-angular-versions) for details.

## Configuration

All options are set under `lsp.angular.initialization_options` in your Zed `settings.json` (or a project-local `.zed/settings.json`):

| Option                         | Type     | Default                                  | Description                                                                        |
| ------------------------------ | -------- | ---------------------------------------- | ---------------------------------------------------------------------------------- |
| `angular_language_server_path` | `string` | `node_modules/@angular/language-server`  | Location of the `@angular/language-server` package directory.                      |
| `max_ts_server_memory`         | `number` | unset (node default, ~4 GB)              | Heap limit in MB, passed to node as `--max-old-space-size`.                         |
| `include_completions_for_module_exports` | `boolean` | `true` | Suggest components that aren't imported yet (adds the import on accept). Set `false` in large projects if completions lag — measured ~8× faster, but only already-imported components are suggested. |
| `completion_proxy_path`        | `string` | unset                                    | Path to the `angular-completion-proxy` binary (see [Completion latency](#completion-latency)). Absolute, `~`-prefixed or worktree-relative. |

Both can be combined — this is the typical monorepo setup, where the app lives in a subfolder *and* the project is large enough to exhaust node's default heap:

```json
{
  "lsp": {
    "angular": {
      "initialization_options": {
        "angular_language_server_path": "client/node_modules/@angular/language-server",
        "max_ts_server_memory": 8192
      }
    }
  }
}
```

Both options are optional and independent; omit either one to keep its default.

### Custom Server Path

Set `angular_language_server_path` when the language server is not installed at the default location — for example in a monorepo where the Angular app lives in a subfolder.

The value must be the **package directory**, not the `index.js` file inside it (a trailing `/index.js` is tolerated and stripped). Accepted forms:

| Form              | Example                                                        | Resolved against                    |
| ----------------- | -------------------------------------------------------------- | ----------------------------------- |
| Worktree-relative | `client/node_modules/@angular/language-server`                 | The root of the open worktree       |
| Absolute          | `/Users/me/repo/client/node_modules/@angular/language-server`  | Used as-is                          |
| Home-relative     | `~/.npm-global/lib/node_modules/@angular/language-server`      | `$HOME` from your shell environment |

The path is **not validated** by the extension. Zed extensions run sandboxed and can only inspect files present in the project's file index, which excludes gitignored trees such as `node_modules`, so any existence check would report false negatives. If the path is wrong, Node reports a `MODULE_NOT_FOUND` error in the language server logs instead.

TypeScript and Angular are then probed in the worktree root, its `node_modules`, and the ancestors of the resolved package directory — so a server under `client/` still resolves `client/node_modules/typescript` correctly.

### Completion latency

After every edit the server recompiles the template before it can answer, and Zed shows the completion menu only once every server has answered. Zed also sends a new completion request on each keystroke; the server can't abort a running compile, so those requests queue up. The optional `angular-completion-proxy` (in `proxy/`) sits in between and — like VS Code — answers completion requests made while you keep typing a word from the request already in flight, instead of queueing another compile. Measured on a 700-line template: ~2.1 s → ~1.0 s.

Install it once per machine (needs Rust) and point the extension at it:

```sh
cargo install --path proxy
```

```json
{
  "lsp": {
    "angular": {
      "initialization_options": {
        "completion_proxy_path": "~/.cargo/bin/angular-completion-proxy"
      }
    }
  }
}
```

Without `completion_proxy_path` the server is started directly. After pulling proxy changes, run `cargo install --path proxy` again and restart the language server.

### Memory

In large workspaces (e.g. monorepos), the language server can exceed node's default heap limit (~4 GB) and crash repeatedly. If the server keeps restarting or stops responding after a few minutes, raise the limit with `max_ts_server_memory` (in MB, passed to node as `--max-old-space-size`):

```json
{
  "lsp": {
    "angular": {
      "initialization_options": {
        "max_ts_server_memory": 8192
      }
    }
  }
}
```

Start at `8192` and increase only if crashes persist; the value is a ceiling, not a reservation, so node allocates lazily. Setting it above the machine's available RAM will trade crashes for swapping. The flag is emitted before the server script path so node interprets it, and it is omitted entirely when the option is unset.
