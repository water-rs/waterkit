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
                            runAndPersistTests(shouldExit: false)
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

            if CommandLine.arguments.contains("--waterkit-run-test") {
                runAndPersistTests(shouldExit: true)
            }
        }
    }

    private func runAndPersistTests(shouldExit: Bool) {
        logger.log("Executing run_tests_json()...")
        DispatchQueue.global(qos: .userInitiated).async {
            let report = run_tests_json().toString()

            do {
                let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                let reportURL = documents.appendingPathComponent("waterkit-test-report.json")
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
