import SwiftUI
import Foundation
import Darwin

struct LogEntry: Identifiable {
    let id = UUID()
    let message: String
    let timestamp = Date()
}

class LogModel: ObservableObject {
    @Published var logs: [LogEntry] = []
    
    func log(_ message: String) {
        DispatchQueue.main.async {
            self.logs.append(LogEntry(message: message))
        }
    }
}

struct ContentView: View {
    @StateObject private var logger = LogModel()
    @State private var autoRunStarted = false
    
    var body: some View {
        NavigationView {
            VStack {
                // Log View
                ScrollView {
                    VStack(alignment: .leading) {
                        ForEach(logger.logs) { entry in
                            Text("[\(entry.timestamp, style: .time)] \(entry.message)")
                                .font(.system(.caption, design: .monospaced))
                                .foregroundColor(.green)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
                }
                .background(Color.black)
                .cornerRadius(8)
                .frame(height: 200)
                
                Divider()
                
                // Test Buttons
                List {
                    Section(header: Text("Tests")) {
                        Button("Run All Tests") {
                            runAndPersistTests(runID: "manual", shouldExit: false)
                        }
                    }
                }
            }
            .padding()
            .navigationTitle("WaterKit Test")
        }
        .onAppear {
            guard !autoRunStarted else {
                return
            }
            autoRunStarted = true

            if let runID = harnessRunID() {
                // A device run can take minutes (camera prompts, frame
                // streams); keep the screen from locking under it, which
                // would interrupt the capture session.
                UIApplication.shared.isIdleTimerDisabled = true
                runAndPersistTests(runID: runID, shouldExit: true)
            }
        }
    }

    /// The run ID the `waterkit-test` runner passes as
    /// `--waterkit-run-test <run-id>`; `nil` when the app was launched by hand.
    private func harnessRunID() -> String? {
        let arguments = CommandLine.arguments
        guard let flag = arguments.firstIndex(of: "--waterkit-run-test") else {
            return nil
        }
        let value = arguments.index(after: flag)
        guard value < arguments.endIndex else {
            fatalError("--waterkit-run-test needs a run ID")
        }
        return arguments[value]
    }

    /// Runs every case and writes the report atomically to
    /// `Documents/waterkit-test-reports/<run-id>.json`. The run ID ties the
    /// file to the launch that asked for it; reports of earlier runs are
    /// removed first so they do not pile up in the container.
    private func runAndPersistTests(runID: String, shouldExit: Bool) {
        logger.log("Executing run_tests_json()...")
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                let reports = documents.appendingPathComponent("waterkit-test-reports", isDirectory: true)
                if FileManager.default.fileExists(atPath: reports.path) {
                    try FileManager.default.removeItem(at: reports)
                }
                try FileManager.default.createDirectory(at: reports, withIntermediateDirectories: true)

                let report = run_tests_json().toString()
                let reportURL = reports.appendingPathComponent("\(runID).json")
                try report.write(to: reportURL, atomically: true, encoding: .utf8)
                logger.log("✓ Wrote structured report")
                if shouldExit {
                    Darwin.exit(0)
                }
            } catch {
                logger.log("✗ Failed to write structured report: \(error)")
                if shouldExit {
                    Darwin.exit(1)
                }
            }
        }
    }
}
