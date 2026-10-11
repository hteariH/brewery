# brewery-daemon

The Kotlin/JVM sidecar of the Brewery IDE. It speaks JSON-RPC over stdio (LSP-style
`Content-Length` framing) and gives the editor a model of the opened project: build
tool, modules and tasks.

## Build

```
./gradlew test shadowJar
```

The fat jar lands in `build/libs/brewery-daemon-all.jar`. Run it with
`java -jar build/libs/brewery-daemon-all.jar`; it needs JDK 21.

## Protocol

The wire protocol is described in the Brewery daemon protocol spec in the repository's
design documents. Example payloads live in `protocol/fixtures/`.

## License

Apache License 2.0, see `LICENSE`.
