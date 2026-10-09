import SwiftUI
import Foundation

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
                            runAndPersistTests()
                        }
                    }
                }
            }
            .padding()
            .navigationTitle("WaterKit Test")
        }
    }

    /// Runs every case and persists the report for inspection with
    /// [`ReportWriter`]. The `waterkit-test` runner drives the suite through
    /// the hosted XCTest instead of this button.
    private func runAndPersistTests() {
        logger.log("Executing run_tests_json()...")
        DispatchQueue.global(qos: .userInitiated).async {
            do {
                let report = run_tests_json().toString()
                try ReportWriter.persist(report)
                logger.log("✓ Wrote structured report")
            } catch {
                logger.log("✗ Failed to write structured report: \(error)")
            }
        }
    }
}
