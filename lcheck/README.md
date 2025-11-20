## Instructions to test linearizability

```bash
# In cr directory, where makefile is present
mkdir -p logs
touch logs/test.txt
stdbuf -oL -eL make node | tee >(stdbuf -oL sed 's/\x1b\[[0-9;]*m//g' > logs/test.txt)
make job
# Wait till everything completes
cd lcheck
go run main.go ../logs/test.txt
```