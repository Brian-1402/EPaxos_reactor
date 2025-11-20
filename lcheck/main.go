package main

import (
	"bufio"
	"fmt"
	"os"
	"regexp"
	"sort"
	"time"
	"path/filepath"
	"strings"
	"strconv"

	"github.com/anishathalye/porcupine"
	"github.com/maruel/natural"
)

type crInputOutput struct {
	op    bool // true = put, false = get
	key   string
	value string
}

// ================= Per-key model =================

var singleKeyModel = porcupine.Model{
	Init: func() interface{} {
		// initial value for one key
		return "NONE"
	},
	Step: func(state, input, output interface{}) (bool, interface{}) {
		in := input.(crInputOutput)
		curr := state.(string)
		if in.op { // put
			return true, in.value
		} else { // get
			out := output.(crInputOutput)
			return out.value == curr, state
		}
	},
	Equal: func(a, b interface{}) bool {
		return a.(string) == b.(string)
	},
	DescribeOperation: func(input, output interface{}) string {
		in := input.(crInputOutput)
		out := output.(crInputOutput)
		if in.op {
			return fmt.Sprintf("put(%v)", in.value)
		}
		return fmt.Sprintf("get()=%v", out.value)
	},
}

// ==================================================
// Revised log parsing (Handles out of order events)
// ==================================================
func parseLog(filename string) []porcupine.Event {
    file, err := os.Open(filename)
    if err != nil {
        panic(err)
    }
    defer file.Close()

    var events []porcupine.Event

    // 1. UPDATED REGEX: Captures ClientID (group 1) and RequestID (group 2)
    // Matches: "... Client_1 [Req:55] Setting key_1 = val"
    reSetterStart := regexp.MustCompile(`Client_?(\d+)\s+\[Req:\s*(\d+)\]\s+Setting\s+(\w+)\s+=\s+(\S*)`)
    reSetterEnd   := regexp.MustCompile(`Client_?(\d+)\s+\[Req:\s*(\d+)\]\s+Set\s+(\w+)\s+=\s+(\S*)`)
    reGetterStart := regexp.MustCompile(`Client_?(\d+)\s+\[Req:\s*(\d+)\]\s+Getting\s+(\w+)(\S*)`)
    reGetterEnd   := regexp.MustCompile(`Client_?(\d+)\s+\[Req:\s*(\d+)\]\s+Get\s+(\w+)\s+=\s+(\S*)`)

    id := 0
    
    // 2. NEW MAP: Maps "ClientID:ReqID" -> Porcupine Event ID
    pendingOps := make(map[string]int)

    scanner := bufio.NewScanner(file)
    for scanner.Scan() {
        line := scanner.Text()

        // Helper to create a unique key for the map (e.g., "1:55")
        makeKey := func(clientId, reqId string) string {
            return clientId + ":" + reqId
        }

        switch {
        // --- WRITER START ---
        case reSetterStart.MatchString(line):
            m := reSetterStart.FindStringSubmatch(line)
            clientId, reqId, key, val := m[1], m[2], m[3], m[4]
            
            // Store the porcupine ID in the map
            pendingOps[makeKey(clientId, reqId)] = id
            
            cid, _ := strconv.Atoi(clientId)
            events = append(events, porcupine.Event{
                ClientId: cid,
                Kind:     porcupine.CallEvent,
                Value:    crInputOutput{true, key, val},
                Id:       id,
            })
            id++

        // --- WRITER END ---
        case reSetterEnd.MatchString(line):
            m := reSetterEnd.FindStringSubmatch(line)
            clientId, reqId, key, val := m[1], m[2], m[3], m[4]

            lookupKey := makeKey(clientId, reqId)
            callId, ok := pendingOps[lookupKey]
            
            if !ok {
				fmt.Printf("Warning: No matching start event for Client %s Req %s\n", clientId, reqId)
				continue
            }
            delete(pendingOps, lookupKey) // Remove from map to keep it clean

            cid, _ := strconv.Atoi(clientId)
            events = append(events, porcupine.Event{
                ClientId: cid,
                Kind:     porcupine.ReturnEvent,
                Value:    crInputOutput{true, key, val},
                Id:       callId, // Links correctly to the specific start event
            })

        // --- READER START ---
        case reGetterStart.MatchString(line):
            m := reGetterStart.FindStringSubmatch(line)
            clientId, reqId, key := m[1], m[2], m[3]

            pendingOps[makeKey(clientId, reqId)] = id

            cid, _ := strconv.Atoi(clientId)
            events = append(events, porcupine.Event{
                ClientId: cid,
                Kind:     porcupine.CallEvent,
                Value:    crInputOutput{false, key, ""},
                Id:       id,
            })
            id++

        // --- READER END ---
        case reGetterEnd.MatchString(line):
            m := reGetterEnd.FindStringSubmatch(line)
            clientId, reqId, key, val := m[1], m[2], m[3], m[4]

            lookupKey := makeKey(clientId, reqId)
            callId, ok := pendingOps[lookupKey]
            if !ok {
				fmt.Printf("Warning: No matching start event for Client %s Req %s\n", clientId, reqId)
				continue
            }
            delete(pendingOps, lookupKey)

            cid, _ := strconv.Atoi(clientId)
            events = append(events, porcupine.Event{
                ClientId: cid,
                Kind:     porcupine.ReturnEvent,
                Value:    crInputOutput{false, key, val},
                Id:       callId,
            })
        }
    }
    return events
}

// ================= Per-key check logic =================

func splitEventsByKey(events []porcupine.Event) map[string][]porcupine.Event {
	grouped := make(map[string][]porcupine.Event)
	for _, e := range events {
		io := e.Value.(crInputOutput)
		grouped[io.key] = append(grouped[io.key], e)
	}
	return grouped
}

func checkLinearizability(filename string) bool {
	fmt.Println("Checking linearizability of log file:", filename)

	events := parseLog(filename)
	if len(events) == 0 {
		fmt.Println("No events found in log file!")
		return false
	}
	
	// 1. Identify which Call IDs actually finished (O(N) pass over the events slice)
	finishedIds := make(map[int]bool)
	for _, ev := range events {
		if ev.Kind == porcupine.ReturnEvent {
			// This ID corresponds to a completed operation
			finishedIds[ev.Id] = true
		}
	}

	// 2. Filter the events list (O(N) pass over the events slice)
	var finalEvents []porcupine.Event
	for _, ev := range events {
		// Keep all Return events (we already used them to populate finishedIds)
		if ev.Kind == porcupine.ReturnEvent {
			finalEvents = append(finalEvents, ev)
			continue
		}

		// Must be a CallEvent at this point
		if finishedIds[ev.Id] {
			// If the Call has a matching Return, keep it
			finalEvents = append(finalEvents, ev)
		}
		// If finishedIds[ev.Id] is false, the call is dangling, and we skip it.
	}

	grouped := splitEventsByKey(finalEvents)

	vizDir := "viz_output"
	// make output dir
	if err := os.MkdirAll(vizDir, 0755); err != nil {
		fmt.Printf("Error creating output directory: %v\n", err)
		os.Exit(1)
	}
	// Get file name without path and extension
	baseName := filepath.Base(filename)
	ext := filepath.Ext(baseName)
	nameOnly := strings.TrimSuffix(baseName, ext)
	outDir := fmt.Sprintf("%s/%s", vizDir, nameOnly)
	if err := os.MkdirAll(outDir, 0755); err != nil {
		fmt.Printf("Error creating run-specific output directory: %v\n", err)
		os.Exit(1)
	}

	// Collect all keys and sort them for consistent output
	// (not strictly necessary, but helps with visualization)
	var keys []string
	for k := range grouped {
		keys = append(keys, k)
	}
	sort.Sort(natural.StringSlice(keys)) // Use natural sorting for better readability

	allOk := true
	for _, key := range keys {
		evs := grouped[key]
		fmt.Printf("=== Checking key %s (%d events) ===\n", key, len(evs))

		// Uncomment below for detailed per-key event debug output
		// // Debug: print events for this key
		// fmt.Printf("DEBUG: Events for key %s:\n", key)
		// for i, e := range evs {
		// 	io := e.Value.(crInputOutput)
		// 	kind := "Call"
		// 	if e.Kind == porcupine.ReturnEvent {
		// 		kind = "Return"
		// 	}
		// 	fmt.Printf("  [%d] Id=%d Proc=%d Kind=%s Key=%s Value=%s\n",
		// 		i, e.Id, e.ClientId, kind, io.key, io.value)
		// }

		// Check linearizability for this key
		res, info := porcupine.CheckEventsVerbose(singleKeyModel, evs, 60*time.Second)
		switch res {
		case porcupine.Ok:
			fmt.Printf("Key %s: linearizable\n", key)
		case porcupine.Illegal:
			fmt.Printf("Key %s: NOT linearizable\n", key)
			allOk = false
		default:
			fmt.Printf("Key %s: check timed out (Unknown)\n", key)
			allOk = false
		}

		// Skip visualization if not linearizable
		if res != porcupine.Ok {
			// fmt.Printf("Skipping visualization for %s because it is NOT linearizable\n", key)
			continue
		}

		// visualization only for linearizable keys
		// per-key viz
		fname := fmt.Sprintf("%s/output_%s.html", outDir, key)
		f, err := os.Create(fname)
		if err != nil {
			fmt.Printf("Error creating visualization file for %s: %v\n", key, err)
			continue
		}
		if err := porcupine.Visualize(singleKeyModel, info, f); err != nil {
			fmt.Printf("Error generating visualization for %s: %v\n", key, err)
		} else {
			fmt.Printf("Visualization for %s written to %s\n", key, fname)
		}
		f.Close()
	}

	if allOk {
		fmt.Println("All keys linearizable")
		// Combined visualization for all keys using manual HTML wrapper (no porcupine method)
		fmt.Println("Generating combined visualization...")
		wrapper := fmt.Sprintf("%s/output_all.html", outDir)
		fw, err := os.Create(wrapper)
		if err != nil {
			fmt.Printf("Error creating wrapper HTML: %v\n", err)
		} else {
			fmt.Fprintln(fw, "<!DOCTYPE html>")
			fmt.Fprintln(fw, "<html><head><title>Combined Visualization</title>")
			fmt.Fprintln(fw, "<style>iframe{width:100%;height:600px;border:1px solid #ccc;margin:10px 0;}</style>")
			fmt.Fprintln(fw, "</head><body>")
			fmt.Fprintln(fw, "<h1>Combined Visualization (per-key)</h1>")
			for _, key := range keys {
				fmt.Fprintf(fw, "<h2>Key %s</h2>\n", key)
				fmt.Fprintf(fw, "<iframe src=\"output_%s.html\"></iframe>\n", key)
			}
			fmt.Fprintln(fw, "</body></html>")
			fw.Close()
			fmt.Printf("Wrapper visualization written to %s\n", wrapper)
		}
	}
	return allOk
}

func main() {
	if len(os.Args) != 2 {
		fmt.Println("Usage: go run main.go <log-file-path>")
		os.Exit(1)
	}

	filename := os.Args[1]
	checkLinearizability(filename)
}
