import SwiftUI

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
                    Section(header: Text("Biometric")) {
                        Button("Test Biometric") {
                            logger.log("Triggering Biometric test...")
                            // test_biometric() // From Rust
                            logger.log("✓ Biometric test triggered")
                        }
                    }
                    
                    Section(header: Text("Location")) {
                        Button("Test Location") {
                            logger.log("Triggering Location test...")
                            logger.log("✓ Location test triggered")
                        }
                    }
                }
            }
            .padding()
            .navigationTitle("WaterKit Test")
        }
    }
}
