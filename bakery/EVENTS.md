# bakery: bread event integration

bakery is a standalone package manager: it works exactly the same with or
without `breadd` running. The app id `bakery` is registered in the `bread`
repository's `KNOWN_APPS`, but this build does not open a `bread-client`
connection, publish events, or subscribe to commands. See the parent `bread`
repo's `Documentation.md`, specifically its "Namespaces" and "Integrating a
bread\* app" sections, for the convention a future integration would follow.

App id: **`bakery`**. Transport: none in this build. This workspace member
is the application root for bakery inside the `bread-ecosystem` monorepo.

## Events published (`bread.bakery.*`)

None.

## Commands honored (`bread.command.bakery.*`)

None.

## Not implemented

- No `bread.bakery.*` event is emitted. Install, update, track, and fetch
  activity stay inside bakery and are not mirrored onto the bus.
- No `bread.command.bakery.*` verb is honored. There is no remote-triggered
  package action in this build.
- Registering the app id is not an integration: it reserves the namespace
  for a future change. Do not add event or command rows here until the code
  that implements them exists.

## Fail-safe behavior

bakery makes no bread socket connections in this build, so there is no bus
path to degrade. Package management is identical whether breadd is
installed, absent, or restarted.
