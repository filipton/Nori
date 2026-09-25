import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputDirectory
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import java.security.MessageDigest
import javax.inject.Inject

plugins {
    alias(libs.plugins.android.library)
}

// Which ABIs the Rust core is built for. Asked explicitly with -PrustTargets; otherwise a debug build is for the
// emulator (x86_64) and a perf or release build for phones (arm64-v8a): building both every time doubled
// each build for a chip nobody was going to run it on.
val shipping = gradle.startParameter.taskNames.any { t -> listOf("release", "perf", "bundle").any { t.contains(it, ignoreCase = true) } }
val rustTargets = (project.findProperty("rustTargets") as String? ?: if (shipping) "arm64-v8a" else "x86_64").split(",")
val rustProfile = project.findProperty("rustProfile") as String? ?: "release"
// Cargo features of the core. `neural-beats` builds in tract for "Better beat detection" (docs/research/analysis.md),
// and the model itself is shipped as an asset (below), so the switch works offline; it stays off by default. The
// debug and perf builds have it; a release build leaves it out unless asked (-PrustFeatures=neural-beats): it
// makes the arm64 APK 20.6 MB bigger (the library 9.8 to 25.3 MB, and the 5.1 MB model). `-PrustFeatures=`
// (empty) leaves it out of any build: no tract in the library, no model in the APK, and no setting shown.
val releasing = gradle.startParameter.taskNames.any { t -> listOf("release", "bundle").any { t.contains(it, ignoreCase = true) } }
val rustFeatures = project.findProperty("rustFeatures") as String? ?: if (releasing) "" else "neural-beats"
val beatModel = rustFeatures.split(",").map { it.trim() }.contains("neural-beats")
val cargoRoot = rootProject.projectDir
val ndkDirPath: String = System.getenv("ANDROID_NDK_HOME")
    ?: file("${System.getenv("ANDROID_HOME") ?: System.getenv("ANDROID_SDK_ROOT") ?: "${System.getProperty("user.home")}/Android/Sdk"}/ndk").listFiles()
        ?.filter { it.isDirectory }?.maxByOrNull { it.name }?.absolutePath
    ?: error("NDK not found; set ANDROID_NDK_HOME")

android {
    namespace = "dev.nori.music.core"
    compileSdk = 37

    defaultConfig {
        minSdk = 26
        ndk { abiFilters += rustTargets }
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

// ---- Rust core -------------------------------------------------------------

abstract class CargoNdkTask @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:InputDirectory @get:PathSensitive(PathSensitivity.RELATIVE) abstract val crates: DirectoryProperty
    @get:InputFile @get:PathSensitive(PathSensitivity.RELATIVE) abstract val cargoToml: RegularFileProperty
    @get:Input abstract val targets: ListProperty<String>
    @get:Input abstract val profile: Property<String>
    @get:Input abstract val features: Property<String>
    @get:Input abstract val ndkDir: Property<String>
    @get:Internal abstract val workDir: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun run() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        val args = mutableListOf("cargo", "ndk")
        targets.get().forEach { args += listOf("-t", it) }
        args += listOf("-o", out.absolutePath, "build", "-p", "nori-android")
        if (profile.get() == "release") args += "--release"
        if (features.get().isNotBlank()) args += listOf("--features", features.get())
        exec.exec {
            workingDir = workDir.get().asFile
            environment("ANDROID_NDK_HOME", ndkDir.get())
            commandLine(args)
        }
    }
}

// The beat model the app ships (tools/beat-this/export.py makes it): copied in as an asset, which the app stores
// uncompressed (app/build.gradle.kts), so the core reads it in place from the APK. Checked against the SHA-256 and
// size pinned in crates/automix/src/beat_model.rs, so the file and the core cannot drift apart.
abstract class BeatModelAssetTask : DefaultTask() {
    @get:InputFile @get:PathSensitive(PathSensitivity.RELATIVE) abstract val model: RegularFileProperty
    @get:InputFile @get:PathSensitive(PathSensitivity.RELATIVE) abstract val pins: RegularFileProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun run() {
        val src = model.get().asFile
        val rs = pins.get().asFile.readText()
        fun pin(name: String) = Regex("""pub const $name: [^=]+= "?([0-9a-f_]+)"?;""").find(rs)?.groupValues?.get(1)?.replace("_", "")
            ?: error("$name not found in ${pins.get().asFile}")
        val bytes = src.readBytes()
        val sha = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        check(sha == pin("SHA256") && bytes.size.toString() == pin("BYTES")) {
            "${src.name} is not the model pinned in beat_model.rs (SHA-256 $sha, ${bytes.size} bytes)"
        }
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        src.copyTo(File(out, src.name))
    }
}

val beatModelAsset = tasks.register<BeatModelAssetTask>("beatModelAsset") {
    group = "rust"
    description = "The beat model, checked against its pin, as an asset"
    model.set(File(cargoRoot, "tools/beat-this/beat-this-small0-v1.onnx"))
    pins.set(File(cargoRoot, "crates/automix/src/beat_model.rs"))
    outputDir.set(layout.buildDirectory.dir("generated/beatModel"))
}

abstract class UniffiBindgenTask @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:InputDirectory @get:PathSensitive(PathSensitivity.RELATIVE) abstract val crates: DirectoryProperty
    @get:Internal abstract val workDir: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun run() {
        // The JNI generator reads the crates' sources (src:), so nothing has to be built for the host first.
        val out = outputDir.get().asFile
        out.deleteRecursively()
        exec.exec {
            workingDir = workDir.get().asFile
            commandLine("cargo", "run", "-q", "-p", "uniffi-bindgen", "--", "bindings", "src:nori-android", out.absolutePath)
        }
    }
}

val cargoNdkBuild = tasks.register<CargoNdkTask>("cargoNdkBuild") {
    group = "rust"
    description = "Cross-compile the Rust core for Android ABIs"
    crates.set(File(cargoRoot, "crates"))
    cargoToml.set(File(cargoRoot, "Cargo.toml"))
    targets.set(rustTargets)
    profile.set(rustProfile)
    features.set(rustFeatures)
    ndkDir.set(ndkDirPath)
    workDir.set(cargoRoot)
    outputDir.set(layout.buildDirectory.dir("rust/jniLibs"))
}

val uniffiBindgen = tasks.register<UniffiBindgenTask>("uniffiBindgen") {
    group = "rust"
    description = "Generate the Kotlin bindings with uniffi-bindgen-kotlin-jni"
    crates.set(File(cargoRoot, "crates"))
    workDir.set(cargoRoot)
    outputDir.set(layout.buildDirectory.dir("generated/uniffi"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.java?.addGeneratedSourceDirectory(uniffiBindgen, UniffiBindgenTask::outputDir)
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoNdkBuild, CargoNdkTask::outputDir)
        if (beatModel) variant.sources.assets?.addGeneratedSourceDirectory(beatModelAsset, BeatModelAssetTask::outputDir)
    }
}

dependencies {
    api(libs.media3.exoplayer)
    api(libs.media3.session)
    implementation(libs.media3.datasource.okhttp)
    // Moving covers are HLS (MotionPlayer); nothing of it is loaded while they are switched off.
    implementation(libs.media3.exoplayer.hls)
    api(libs.okhttp)
    api(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.coroutines.guava)
    implementation(libs.androidx.core.ktx)
    testImplementation("junit:junit:4.13.2")
}
