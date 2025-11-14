## Run commands
- `make build`: Builds the project
- `make node`: Runs the node http server at port 3000. Builds the project as well
- `make job`: (after make node) Runs job controller to load the epaxos library, connect to node and starts epaxos

## CI/CD check commands
```bash
# Check formatting issues
cargo fmt --all -- --check

# Run clippy (linter) for post-compilation code suggestions and fixes
cargo clippy -- -D warnings
```

### Auto-fix commands for the above checks
```bash 
# Format entire codebase
cargo fmt --all

# Attempts to fix the issues suggested by clippy earlier (doesn't do all the fixes)
cargo clippy --fix --allow-dirty --allow-staged -- -A warnings
```

## Info
- For now just made a single server responding to reads
