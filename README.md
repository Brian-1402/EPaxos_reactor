## Run commands
- `make build`: Builds the project
- `make node`: Runs the node http server at port 3000. Builds the project as well
- `make job`: (after make node) Runs job controller to load the epaxos library, connect to node and starts epaxos
### Extra commands
- `cargo fmt --all`: Format entire codebase
- `cargo clippy -- -D warnings`: Run clippy for post-compilation code suggestions and fixes

## Info
- For now just made a single server responding to reads
