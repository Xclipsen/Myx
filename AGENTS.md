# Project Instructions

## TUI First

- This is a TUI-first project. Design and implement features for the terminal interface first.
- Keep interactions keyboard-friendly, responsive, and clear within typical terminal constraints.
- Do not introduce a GUI or web interface unless the user explicitly requests it.

## Branches and Pull Requests

- Do not create a feature branch or pull request unless the user explicitly asks for it.
- When no branch or pull request was requested, make changes only in the current working tree.
- Before creating a requested pull request, ensure the branch is up to date and all merge or rebase conflicts are fully resolved.
- Never leave conflict markers or unresolved conflicts in the working tree.

## Personal Main and Testing Builds

- `origin` is the upstream repository (`HaseebKhalid1507/Myx`); `fork` is the user's personal repository (`Xclipsen/Myx`).
- `fork/main` is the user's personal integration branch. It may contain tested features that are not proposed to upstream and do not have an upstream pull request.
- When the user says "main", "auf main", or "mein Main-MYX", integrate the requested work into `fork/main`, run the full relevant verification suite, and install that result as the `myx` executable.
- Never infer an upstream pull request from a feature being present on `fork/main`. Open or update an upstream pull request only when the user explicitly asks for one.
- Keep a separate `myx-testing` executable for experimental feature builds. Updating `myx-testing` must not replace the installed `myx` build unless the user explicitly promotes the feature to personal main.
- When the user says a feature should stay "local", keep it uncommitted and unpushed in the local working tree, install it only as `myx-testing`, and do not create or update any pull request unless the user later explicitly asks for promotion.
- Before promoting work to personal main, bring in the latest compatible upstream main, preserve personal-only features, resolve every conflict, and run formatting, Clippy, tests, and applicable feature builds.
- On sufficiently wide terminals, personal main keeps the Now Playing layout with the library on the left, playback in the center, and the live queue on the right; narrow terminals retain the responsive single-pane fallback.

## Lightweight Development

- Prefer small, focused changes with minimal complexity and resource usage.
- Reuse existing project patterns and dependencies wherever practical.
- Avoid unnecessary dependencies, abstractions, background processes, and broad refactors.
- Keep implementations easy to understand, maintain, and run in a terminal environment.
