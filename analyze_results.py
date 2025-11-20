import re
import glob
import os
from datetime import datetime
from collections import defaultdict
import matplotlib.pyplot as plt

# --- Configuration ---
LOG_FILE_PATTERN = "./logs/logfile_*rps.log"
TIMESTAMP_FORMAT = "%Y-%m-%dT%H:%M:%S.%fZ"

def parse_log_file(filepath):
    """
    Parses a single log file to extract the target RPS (from filename) 
    and the request latencies (from log entries), filtering entries
    to only include those that occur between 30 and 80 seconds after
    the first timestamp found in the log file.
    """
    # 1. Extract Target RPS from filename (e.g., logfile_10rps.log -> 10)
    filename = os.path.basename(filepath)
    rps_match = re.search(r'logfile_(\d+)rps\.log', filename)
    if not rps_match:
        print(f"Warning: Could not extract RPS from filename: {filename}. Skipping.")
        return None, []
    
    rps = int(rps_match.group(1))
    
    # Regex to capture the key log elements:
    log_regex = re.compile(
        r"^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{6}Z).*\s+(Client_\d+)\s+\[Req:\s+(\d+)\]\s+(Setting|Set)\s+"
    )
    
    # Store start timestamps: Key is (client_id, req_id)
    request_starts = {} 
    latencies_ms = []
    
    # New variable to store the absolute start time of the test run
    run_start_time = None 

    print(f"--- Parsing {filename} (Target RPS: {rps}) with 30-80s filter ---")

    try:
        with open(filepath, 'r') as f:
            for line in f:
                match = log_regex.search(line)
                if match:
                    timestamp_str, client_id, req_id_str, action = match.groups()
                    
                    try:
                        # Convert timestamp string to datetime object
                        timestamp = datetime.strptime(timestamp_str, TIMESTAMP_FORMAT)
                    except ValueError:
                        print(f"Error parsing timestamp in line: {line.strip()}. Skipping entry.")
                        continue
                    
                    # --- NEW TIME-WINDOW LOGIC ---
                    if run_start_time is None:
                        # Set the absolute start time to the first valid timestamp
                        run_start_time = timestamp
                    
                    # Calculate elapsed time in seconds from the start of the log file
                    elapsed_time_s = (timestamp - run_start_time).total_seconds()
                    
                    # Check if the entry is within the 30s to 80s window
                    # We will only process requests that START within this window, 
                    # but allow their 'Set' (completion) entries to be processed 
                    # even if they fall outside, as long as the 'Setting' (start) 
                    # entry was recorded.
                    
                    # If the entry is too early, skip it.
                    if elapsed_time_s < 30 and action == "Setting":
                        continue
                        
                    # If the entry is too late, we can stop processing this log file.
                    # We continue briefly to process any 'Set' entries for requests
                    # that started just before 80s, but we don't start new ones.
                    if elapsed_time_s > 85 and action == "Setting":
                        # We use 85s as a small buffer, but requests starting after 80s 
                        # are the main target for exclusion. 
                        # A better check is to only record 'Setting' if < 80s
                        pass # The logic below handles this better
                    # --- END OF NEW TIME-WINDOW LOGIC ---

                    req_id = int(req_id_str)
                    key = (client_id, req_id)

                    if action == "Setting":
                        # Only record the start if it is within the desired window (0-80s)
                        if elapsed_time_s < 80:
                            request_starts[key] = timestamp
                    
                    elif action == "Set":
                        # This is the completion of a request
                        if key in request_starts:
                            start_time = request_starts.pop(key)
                            latency = (timestamp - start_time).total_seconds() * 1000 # Convert to milliseconds
                            latencies_ms.append(latency)
                        # Else: the start of the request wasn't captured (likely because it
                        # occurred before 30s or was too late, or was simply missed), ignore the end.

    except FileNotFoundError:
        print(f"Error: File not found at {filepath}")
        return None, []
    except Exception as e:
        print(f"An unexpected error occurred while processing {filepath}: {e}")
        return None, []
        
    print(f"Found {len(latencies_ms)} completed requests within the 30-80s window.")
    return rps, latencies_ms

def generate_latency_graph(performance_data):
    """
    Generates a Latency vs. Throughput (RPS) graph.
    performance_data is a dictionary of {RPS: average_latency_ms}.
    """
    if not performance_data:
        print("No valid data found to generate a graph.")
        return

    # 1. Prepare data for plotting
    # Sort the data by RPS for a clean graph
    sorted_data = sorted(performance_data.items())
    
    # Unzip the sorted data into two lists
    throughputs = [item[0] for item in sorted_data]
    avg_latencies = [item[1] for item in sorted_data]

    # 2. Create the plot
    plt.figure(figsize=(10, 6))
    
    # Plot the line graph (Using a marker helps show the data points clearly)
    plt.plot(throughputs, avg_latencies, marker='o', linestyle='-', color='indigo', linewidth=2, markersize=8)
    
    # Add labels and title
    plt.title('Average Request Latency vs. System Throughput', fontsize=16, pad=20)
    plt.xlabel('Throughput (Requests per Second - RPS)', fontsize=14)
    plt.ylabel('Average Latency (ms)', fontsize=14)
    
    # Add grid for readability
    plt.grid(True, linestyle='--', alpha=0.6)
    
    # Annotate each point with its average latency
    for rps, lat in zip(throughputs, avg_latencies):
        plt.annotate(f'{lat:.2f} ms', 
                     (rps, lat), 
                     textcoords="offset points", 
                     xytext=(5,-15), 
                     ha='center',
                     fontsize=10)

    # Set axes limits to start from 0 for better representation
    plt.xlim(xmin=0)
    plt.ylim(ymin=0)

    # Enhance tick appearance
    plt.xticks(throughputs) # Ensure only the measured RPS values are shown on the axis
    
    # Show the plot
    plt.tight_layout()
    print("\nSuccessfully generated the graph.")
    plt.show()

def main():
    """Main function to find files, parse them, and generate the final plot."""
    
    # Find all log files matching the pattern in the current directory
    log_files = glob.glob(LOG_FILE_PATTERN)
    
    if not log_files:
        print(f"Error: No log files found matching '{LOG_FILE_PATTERN}' in the current directory.")
        print("Please ensure your log files (e.g., logfile_10rps.log) are in the same folder as this script.")
        return

    # Dictionary to store {RPS: [latency1, latency2, ...]}
    all_performance_data = defaultdict(list)
    
    for file_path in log_files:
        rps, latencies = parse_log_file(file_path)
        
        if rps is not None and latencies:
            all_performance_data[rps].extend(latencies)

    # Calculate average latency for each RPS
    average_performance_data = {}
    for rps, latencies in all_performance_data.items():
        if latencies:
            avg_latency = sum(latencies) / len(latencies)
            average_performance_data[rps] = avg_latency
            print(f"RPS {rps}: Average Latency = {avg_latency:.2f} ms ({len(latencies)} samples)")
        
    # Generate the plot
    generate_latency_graph(average_performance_data)

if __name__ == "__main__":
    main()