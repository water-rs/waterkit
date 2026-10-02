plugins {
    id("com.android.application")
}

// waterkit crates declare their Android classpath in
// `[package.metadata.waterui.android]` — `kotlin-sources` paths relative to
// the crate manifest and `maven` coordinates — the same channel the water
// CLI's classpath staging consumes (`scan_android_sources` in water-rs/cli).
// The harness resolves those declarations through `cargo metadata` against
// the feature set the waterkit-test driver builds the harness library with
// (passed here as `-PwaterkitFeatures=<feature>`), so a helper only compiles
// into the test app when its crate and gate feature are in the resolved
// graph — dead or undeclared sources never reach the DEX.
val waterkitFeatures = providers.gradleProperty("waterkitFeatures").orNull
    ?: error(
        "missing -PwaterkitFeatures=<feature>: the waterkit-test driver passes " +
            "the cargo feature it builds the harness library with"
    )

val cargoMetadataOutput = providers.exec {
    isIgnoreExitValue = true
    commandLine(
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--manifest-path",
        rootProject.projectDir.resolve("rust/Cargo.toml").absolutePath,
        "--features",
        waterkitFeatures,
    )
}
val cargoMetadataJson = cargoMetadataOutput.standardOutput.asText.get()
if (cargoMetadataOutput.result.get().exitValue != 0) {
    error("cargo metadata failed: ${cargoMetadataOutput.standardError.asText.get()}")
}

val cargoMetadata = groovy.json.JsonSlurper().parseText(cargoMetadataJson) as Map<*, *>
val enabledFeatures =
    ((cargoMetadata["resolve"] as Map<*, *>)["nodes"] as List<*>)
        .associate { node ->
            node as Map<*, *>
            node["id"] to (node["features"] as List<*>).toSet()
        }

val kotlinSources = linkedMapOf<String, File>()
val mavenCoordinates = linkedSetOf<String>()
for (pkg in cargoMetadata["packages"] as List<*>) {
    pkg as Map<*, *>
    val waterui = (pkg["metadata"] as? Map<*, *>)?.get("waterui") as? Map<*, *> ?: continue
    val android = waterui["android"] as? Map<*, *> ?: continue
    val requiredFeature = android["required-feature"] as? String
    if (requiredFeature != null && enabledFeatures[pkg["id"]]?.contains(requiredFeature) != true) {
        continue
    }
    val crateRoot = File(pkg["manifest_path"] as String).parentFile
    for (source in android["kotlin-sources"] as? List<*> ?: emptyList<Any>()) {
        val file = File(crateRoot, source as String)
        require(file.isFile) {
            "crate ${pkg["name"]} declares Kotlin source $source that does not exist"
        }
        val previous = kotlinSources.put(file.name, file)
        require(previous == null || previous == file) {
            "two crates declare a Kotlin source named ${file.name}"
        }
    }
    for (coordinate in android["maven"] as? List<*> ?: emptyList<Any>()) {
        mavenCoordinates += coordinate as String
    }
}

// Stage the declared sources into a build-dir directory, mirroring the CLI's
// `src/main/java/waterui/` staging, so only declared files compile.
val stagedHelpers = layout.buildDirectory.dir("waterkit-classpath/java").get().asFile
stagedHelpers.listFiles()?.forEach { stale ->
    if (stale.name !in kotlinSources.keys) {
        stale.delete()
    }
}
stagedHelpers.mkdirs()
kotlinSources.forEach { (name, source) ->
    source.copyTo(stagedHelpers.resolve(name), overwrite = true)
}

android {
    namespace = "com.waterkit.test"
    compileSdk = 37

    defaultConfig {
        // Overridable for parallel installs of the harness (e.g. a focused
        // single-feature build next to the full harness on one device):
        // ./gradlew :app:assembleDebug -PwaterkitApplicationId=com.example.other
        applicationId = providers.gradleProperty("waterkitApplicationId").getOrElse("com.waterkit.test")
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "1.0"

        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            jniLibs.directories.add("src/main/jniLibs")
            // .kt files compile from the `kotlin` source set, not `java` —
            // adding the staged dir to `java.directories` leaves the helpers
            // out of the DEX and every runtime loadClass fails.
            kotlin.directories.add(stagedHelpers.absolutePath)
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.19.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("com.google.android.material:material:1.14.0")
    implementation("androidx.activity:activity-ktx:1.13.0")
    // Maven coordinates waterkit crates declare for their helpers
    // ([package.metadata.waterui.android] maven entries).
    mavenCoordinates.forEach { implementation(it) }
}
