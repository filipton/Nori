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
import javax.inject.Inject

plugins {
    alias(libs.plugins.android.library)
}

val rustTargets = (project.findProperty("rustTargets") as String? ?: "arm64-v8a,x86_64").split(",")
val rustProfile = project.findProperty("rustProfile") as String? ?: "release"
val cargoRoot = rootProject.projectDir
val ndkDirPath: String = System.getenv("ANDROID_NDK_HOME")
    ?: file("${System.getenv("ANDROID_HOME") ?: System.getenv("ANDROID_SDK_ROOT") ?: "${System.getProperty("user.home")}/Android/Sdk"}/ndk").listFiles()
        ?.filter { it.isDirectory }?.maxByOrNull { it.name }?.absolutePath
    ?: error("NDK not found; set ANDROID_NDK_HOME")

android {
    namespace = "dev.flint.music.core"
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
        args += listOf("-o", out.absolutePath, "build", "-p", "flintmusic")
        if (profile.get() == "release") args += "--release"
        exec.exec {
            workingDir = workDir.get().asFile
            environment("ANDROID_NDK_HOME", ndkDir.get())
            commandLine(args)
        }
    }
}

abstract class UniffiBindgenTask @Inject constructor(private val exec: ExecOperations) : DefaultTask() {
    @get:InputDirectory @get:PathSensitive(PathSensitivity.RELATIVE) abstract val crates: DirectoryProperty
    @get:Internal abstract val workDir: DirectoryProperty
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun run() {
        val wd = workDir.get().asFile
        exec.exec {
            workingDir = wd
            commandLine("cargo", "build", "-q", "-p", "flintmusic")
        }
        exec.exec {
            workingDir = wd
            commandLine(
                "cargo", "run", "-q", "-p", "uniffi-bindgen", "--",
                "generate", "--library", "target/debug/libflintmusic.so",
                "--language", "kotlin", "--no-format",
                "--out-dir", outputDir.get().asFile.absolutePath,
            )
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
    ndkDir.set(ndkDirPath)
    workDir.set(cargoRoot)
    outputDir.set(layout.buildDirectory.dir("rust/jniLibs"))
}

val uniffiBindgen = tasks.register<UniffiBindgenTask>("uniffiBindgen") {
    group = "rust"
    description = "Generate Kotlin bindings with uniffi"
    crates.set(File(cargoRoot, "crates"))
    workDir.set(cargoRoot)
    outputDir.set(layout.buildDirectory.dir("generated/uniffi"))
}

androidComponents {
    onVariants { variant ->
        variant.sources.java?.addGeneratedSourceDirectory(uniffiBindgen, UniffiBindgenTask::outputDir)
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoNdkBuild, CargoNdkTask::outputDir)
    }
}

dependencies {
    api(libs.media3.exoplayer)
    api(libs.media3.session)
    implementation(libs.media3.datasource.okhttp)
    api(libs.okhttp)
    api(libs.kotlinx.coroutines.android)
    implementation(libs.kotlinx.coroutines.guava)
    implementation(libs.androidx.core.ktx)
    implementation("${libs.jna.get()}@aar")
    testImplementation("junit:junit:4.13.2")
}
