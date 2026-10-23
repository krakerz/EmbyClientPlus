/*
 * Stand-in for VapourSynth's libraries, under the names libmpv was linked
 * against: bundled next to libmpv on Windows and macOS, and on Linux in a
 * folder of its own on the app's library path (build.rs, the launcher).
 * libmpv needs them to start at all, but VapourSynth only matters once SVP
 * attaches.
 *
 * The app picks the VSScript library (SVP's own, or with the "System"
 * preference an installed one) and passes its full path in
 * EMBYCLIENTPLUS_VSSCRIPT before libmpv initialises, plus SVP's in
 * EMBYCLIENTPLUS_VSSCRIPT_FALLBACK when the first one may fail. The first
 * call here loads it (with its own folder's dependencies) and forwards to
 * it. Without one the calls return NULL and mpv plays without the
 * vapoursynth filter.
 *
 * Build: cc -shared -O2 -o <name> vsshim.c (packaging/package-windows.sh);
 * on Linux with -fPIC -Wl,-soname,libvsscript.so ... -ldl (build.rs,
 * packaging/package.sh).
 */

#ifdef __linux__
#define _GNU_SOURCE /* dl_iterate_phdr */
#endif

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <windows.h>
/* The app sets these at startup with SetEnvironmentVariable, after this
 * library (and its C runtime's copy of the environment, which getenv reads)
 * was loaded: ask Windows for the live value instead. */
static const char *env(const char *name)
{
    static char values[2][2048];
    static int next;
    char *value = values[next++ % 2];
    DWORD len = GetEnvironmentVariableA(name, value, sizeof values[0]);
    return len > 0 && len < sizeof values[0] ? value : NULL;
}
#else
static const char *env(const char *name)
{
    return getenv(name);
}
#endif

/* Appends a line to EMBYCLIENTPLUS_VSSHIM_LOG (set by the app, in its
 * logs folder): this library has no other way to say why it failed. */
static void note(const char *format, ...)
{
    const char *path = env("EMBYCLIENTPLUS_VSSHIM_LOG");
    if (!path || !*path)
        return;
    FILE *file = fopen(path, "a");
    if (!file)
        return;
    va_list args;
    va_start(args, format);
    vfprintf(file, format, args);
    va_end(args);
    fputc('\n', file);
    fclose(file);
}

#ifdef _WIN32
#include <windows.h>
#define EXPORT __declspec(dllexport)
typedef HMODULE lib_t;
static lib_t open_lib(const char *path)
{
    /* Altered search path: the library's own dependencies (VapourSynth,
     * Python) resolve from its folder rather than ours. */
    return LoadLibraryExA(path, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
}
static void *find_symbol(lib_t lib, const char *name)
{
    return (void *)GetProcAddress(lib, name);
}
static void note_error(const char *what, const char *path)
{
    DWORD code = GetLastError();
    char message[512] = "";
    FormatMessageA(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS, NULL, code, 0,
                   message, sizeof message, NULL);
    note("%s %s failed: error %lu: %s", what, path, (unsigned long)code, message);
}
#define SEP '\\'
#define CORE_NAME "VapourSynth.dll"
#else
#include <dlfcn.h>
#ifdef __linux__
#include <link.h>
#endif
#define EXPORT __attribute__((visibility("default")))
typedef void *lib_t;
static lib_t open_lib(const char *path)
{
    return dlopen(path, RTLD_NOW | RTLD_GLOBAL);
}
static void *find_symbol(lib_t lib, const char *name)
{
    return dlsym(lib, name);
}
static void note_error(const char *what, const char *path)
{
    note("%s %s failed: %s", what, path, dlerror());
}
#define SEP '/'
#ifdef __APPLE__
#define CORE_NAME "libvapoursynth.dylib"
#else
#define CORE_NAME "libvapoursynth.so"
#endif
#endif

typedef const void *(*api_fn)(int);

#ifdef _WIN32
/* SVP's pip-style VapourSynth loads python3.dll, a forwarder to
 * python3XY.dll that Windows must find by name. Our program isn't in that
 * folder, so the Python DLLs beside `script` are loaded by full path first;
 * the forwarder then finds them already in memory. */
static void preload_python(const char *script)
{
    const char *slash = strrchr(script, '\\');
    if (!slash)
        return;
    size_t dir = (size_t)(slash - script + 1);
    char pattern[MAX_PATH], path[MAX_PATH];
    if (dir + 16 >= MAX_PATH)
        return;
    memcpy(pattern, script, dir);
    strcpy(pattern + dir, "python3*.dll");
    WIN32_FIND_DATAA found;
    HANDLE search = FindFirstFileA(pattern, &found);
    if (search == INVALID_HANDLE_VALUE)
        return;
    do {
        if (dir + strlen(found.cFileName) + 1 > MAX_PATH)
            continue;
        memcpy(path, script, dir);
        strcpy(path + dir, found.cFileName);
        if (open_lib(path))
            note("loaded %s", path);
        else
            note_error("loading", path);
    } while (FindNextFileA(search, &found));
    FindClose(search);
}
#else
static void preload_python(const char *script)
{
    (void)script;
}
#endif

/* The VSScript library being loaded or in use; VapourSynth's core library
 * is looked for beside it. */
static char current_script[2048];
/* Set once that library's API worked: what the app shows as in use. */
static char loaded_script[2048];
static const char *loaded;

/* For the app: the VSScript library in use, or NULL (none yet, or none
 * worked). Its presence also tells the app this stand-in is in use. */
EXPORT const char *embyclientplus_vsshim_loaded(void)
{
    return __atomic_load_n(&loaded, __ATOMIC_ACQUIRE);
}

/* Copies environment variable `name` into `out`; 0 when unset or too long. */
static int copy_env(const char *name, char *out, size_t size)
{
    const char *value = env(name);
    if (!value || !*value || strlen(value) >= size) {
        out[0] = '\0';
        return 0;
    }
    strcpy(out, value);
    return 1;
}

/* Asks `lib` for its API. Versions are (major << 16) | minor. An older
 * VapourSynth (SVP for Windows ships R64) refuses a newer minor than it
 * has; minors only add entries at the end, so the older table serves what
 * mpv uses. */
static const void *call_api(lib_t lib, const char *symbol, int version)
{
    api_fn fn = (api_fn)find_symbol(lib, symbol);
    if (!fn) {
        note("%s not found in the loaded library", symbol);
        return NULL;
    }
    int major = version & ~0xffff;
    for (int minor = version & 0xffff; minor >= 0; minor--) {
        const void *api = fn(major | minor);
        if (api) {
            note("%s(%d.%d) succeeded", symbol, major >> 16, minor);
            return api;
        }
        note("%s(%d.%d) returned nothing", symbol, major >> 16, minor);
    }
    /* VSScript's own reason, where it has one (R70+). */
    typedef const char *(*error_fn)(void);
    error_fn last_error = (error_fn)find_symbol(lib, "getVSScriptAPILastError");
    if (last_error && last_error())
        note("VapourSynth says: %s", last_error());
    return NULL;
}

/* Whether a Python runtime is in the process already. A VSScript library
 * that failed after starting Python leaves it loaded, and a second Python
 * (the fallback's) would bind to the first one's symbols. */
#ifdef _WIN32
static int python_loaded(void)
{
    char name[16];
    if (GetModuleHandleA("python3.dll"))
        return 1;
    for (int minor = 6; minor <= 20; minor++) {
        snprintf(name, sizeof name, "python3%d.dll", minor);
        if (GetModuleHandleA(name))
            return 1;
    }
    return 0;
}
#elif defined(__linux__)
static int is_python(struct dl_phdr_info *info, size_t size, void *data)
{
    (void)size;
    (void)data;
    const char *slash = strrchr(info->dlpi_name, '/');
    return strncmp(slash ? slash + 1 : info->dlpi_name, "libpython", 9) == 0;
}
static int python_loaded(void)
{
    return dl_iterate_phdr(is_python, NULL) != 0;
}
#else
static int python_loaded(void)
{
    return 0;
}
#endif

#ifdef _WIN32
/* SVP's VapourSynth finds Python through PATH (the app puts SVP's folder
 * there only when SVP's library comes first), so the fallback's folder goes
 * first on PATH before it loads. */
static void prepend_to_path(const char *library)
{
    static char path[32768];
    const char *slash = strrchr(library, '\\');
    if (!slash)
        return;
    size_t dir = (size_t)(slash - library);
    if (dir + 2 >= sizeof path)
        return;
    memcpy(path, library, dir);
    path[dir] = ';';
    DWORD room = (DWORD)(sizeof path - dir - 1);
    DWORD len = GetEnvironmentVariableA("PATH", path + dir + 1, room);
    if (len >= room)
        return;
    if (len == 0)
        path[dir] = '\0';
    SetEnvironmentVariableA("PATH", path);
}
#else
static void prepend_to_path(const char *library)
{
    (void)library;
}
#endif

/* Loads VSScript library `script` and asks it for the API. */
static const void *try_script(const char *script, lib_t *lib, int version)
{
    strcpy(current_script, script); /* both come from copy_env: < 2048 */
    preload_python(script);
    *lib = open_lib(script);
    if (!*lib) {
        note_error("loading", script);
        return NULL;
    }
    note("loaded %s", script);
    const void *api = call_api(*lib, "getVSScriptAPI", version);
    if (api) {
        strcpy(loaded_script, script);
        __atomic_store_n(&loaded, loaded_script, __ATOMIC_RELEASE);
    }
    return api;
}

static lib_t script_lib;
static int script_tried;

EXPORT const void *getVSScriptAPI(int version)
{
    if (script_tried)
        return script_lib ? call_api(script_lib, "getVSScriptAPI", version) : NULL;
    script_tried = 1;
    char primary[2048], fallback[2048];
    if (!copy_env("EMBYCLIENTPLUS_VSSCRIPT", primary, sizeof primary)) {
        note("EMBYCLIENTPLUS_VSSCRIPT isn't set: no VapourSynth found");
        return NULL;
    }
    lib_t lib = NULL;
    const void *api = try_script(primary, &lib, version);
    if (!api && copy_env("EMBYCLIENTPLUS_VSSCRIPT_FALLBACK", fallback, sizeof fallback)) {
        if (python_loaded()) {
            note("%s failed after starting Python: not trying %s until the app restarts",
                 primary, fallback);
        } else {
            note("%s failed: falling back to %s", primary, fallback);
            prepend_to_path(fallback);
            api = try_script(fallback, &lib, version);
        }
    }
    if (api)
        script_lib = lib;
    return api;
}

/* Not on Linux: there Python's vapoursynth module would bind to this one
 * (the stand-in is in the global scope first) instead of its own core,
 * which pip-style VapourSynth names libvapoursynth.so.4, not CORE_NAME.
 * libmpv on Linux needs only getVSScriptAPI. */
#ifndef __linux__
/* Loads `name` from the folder of `script`. */
static lib_t load_beside(const char *script, const char *name)
{
    const char *slash = strrchr(script, SEP);
    size_t dir = slash ? (size_t)(slash - script + 1) : 0;
    char *path = malloc(dir + strlen(name) + 1);
    if (!path)
        return NULL;
    memcpy(path, script, dir);
    strcpy(path + dir, name);
    lib_t lib = open_lib(path);
    if (lib)
        note("loaded %s", path);
    else
        note_error("loading", path);
    free(path);
    return lib;
}

static lib_t core_lib;
static char core_for[2048];

EXPORT const void *getVapourSynthAPI(int version)
{
    if (!current_script[0]
        && !copy_env("EMBYCLIENTPLUS_VSSCRIPT", current_script, sizeof current_script)) {
        note("EMBYCLIENTPLUS_VSSCRIPT isn't set: no VapourSynth found");
        return NULL;
    }
    /* Again when the fallback took over from the first library. */
    if (strcmp(core_for, current_script) != 0) {
        strcpy(core_for, current_script);
        core_lib = load_beside(current_script, CORE_NAME);
    }
    return core_lib ? call_api(core_lib, "getVapourSynthAPI", version) : NULL;
}
#endif
