# Open the installed FireTV app

`canvas-fleet launch firetv` sends the narrow correlated owner-local `launch`
request to the configured `canvas-tvctl SERIAL` adapter. Update both CLI binaries;
no TV APK upgrade is required. ADB must already be reachable and authorized.

The adapter resolves `android.intent.action.MAIN` with the
`android.intent.category.LEANBACK_LAUNCHER` category for `dev.fpl.canvas.tv`,
validates the resulting component belongs to that package, then calls
`am start -W --activity-single-top -n COMPONENT`. It polls current window focus
for up to three seconds. Success reports the exact activity and
`foreground: true`; failure stays visible as a correlated error receipt.

The activity class is deliberately not inferred from the package name. No
arbitrary package/command arguments, installation, data clearing, force-stop,
workspace selection, or new gateway permissions are introduced. Fleet bounds
the complete owner invocation to twelve seconds. Foreground focus is not proof
that every widget has loaded or every media source is reachable.
