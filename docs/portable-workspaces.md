# Portable Canvas workspaces

Canvas owns workspace documents, persistence, layout, focus, and output
assignment. `canvas-edge` transports complete validated generations over the
existing `canvas.workspace_state.v1` Unibus contract. Mycelium may provide the
route used to reach a host for installation, but it is not a runtime dependency
of workspace movement.

The household outputs have stable identities:

| Canvas | Output ID |
| --- | --- |
| MacBook Neo | `screen-neo` |
| Mac Studio | `screen-studio` |
| Home Pi television | `screen-tv` |

Every Canvas retains an imported workspace. Only the Canvas whose output ID
matches `assigned_output` presents it. Assigning another workspace to an output
atomically displaces the previous assignment; the displaced Canvas recalls its
most recent local workspace, or a generated empty `canvas-idle` workspace when
none exists.

## Operator runbook

List retained workspaces and their assignments:

```sh
echo 'workspace list' | canvasctl
echo 'workspace list --output screen-tv' | canvasctl
```

Move a retained workspace to an output:

```sh
echo 'workspace summon wikiracing --output screen-tv' | canvasctl
echo 'workspace summon studio --output screen-studio' | canvasctl
echo 'workspace summon security --output screen-neo' | canvasctl
```

`workspace move` is an alias for `workspace summon`. The command reads the
stored document without briefly presenting it, records the assignment locally,
and hands one complete snapshot to the local edge. Each peer imports that
snapshot; only the target activates it.

## Verification

On every node, `workspace list` should show the same imported workspace and
assignment. On the target, it should also say `active`. Moving it again should
remove the old assignment, present it on the new target, and leave the previous
output on its last local workspace.

If movement fails, inspect `canvas-edge` logs first. Unsupported documents,
unknown output IDs, store-write failures, and rejected control messages are
reported explicitly; there is no silent local-only fallback.
