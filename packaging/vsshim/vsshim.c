/*
 * Stand-in for VapourSynth's libraries, bundled next to libmpv on Windows
 * and macOS under the names libmpv was linked against. libmpv needs them
 * to start at all, but VapourSynth only matters once SVP attaches, and
 * then it must be SVP's own copy (its plugins are built against it).
 *
 * The app finds SVP's VSScript library and passes its full path in
 * EMBYCLIENTPLUS_VSSCRIPT before libmpv initialises; the first call here
 * loads it (with its own folder's dependencies) and forwards to it. Without
 * SVP the calls return NULL and mpv plays without the vapoursynth filter.
 *
 * Build: cc -shared -O2 -o <name> vsshim.c (see packaging/package-*.sh).
 */

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

/* Loads `name` from the folder of SVP's VSScript library (or that library
 * itself when `name` is NULL), once. */
static lib_t load(const char *name)
{
    /* Copied: env() reuses its buffers, and note() calls it again. */
    static char script[2048];
    const char *value = env("EMBYCLIENTPLUS_VSSCRIPT");
    if (value && strlen(value) < sizeof script)
        strcpy(script, value);
    else
        script[0] = '\0';
    if (!*script) {
        note("EMBYCLIENTPLUS_VSSCRIPT isn't set: no VapourSynth found");
        return NULL;
    }
    if (!name) {
        preload_python(script);
        lib_t lib = open_lib(script);
        if (lib)
            note("loaded %s", script);
        else
            note_error("loading", script);
        return lib;
    }
    const char *slash = strrchr(script, SEP);
    size_t dir = slash ? (size_t)(slash - script + 1) : 0;
    size_t len = dir + strlen(name) + 1;
    char *path = malloc(len);
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

static const void *forward(lib_t *lib, int *tried, const char *name, const char *symbol,
                           int version)
{
    if (!*tried) {
        *tried = 1;
        *lib = load(name);
    }
    if (!*lib)
        return NULL;
    api_fn fn = (api_fn)find_symbol(*lib, symbol);
    if (!fn) {
        note("%s not found in the loaded library", symbol);
        return NULL;
    }
    /* Versions are (major << 16) | minor. An older VapourSynth (SVP for
     * Windows ships R64) refuses a newer minor than it has; minors only
     * add entries at the end, so the older table serves what mpv uses. */
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
    error_fn last_error = (error_fn)find_symbol(*lib, "getVSScriptAPILastError");
    if (last_error && last_error())
        note("VapourSynth says: %s", last_error());
    return NULL;
}

static lib_t script_lib, core_lib;
static int script_tried, core_tried;

EXPORT const void *getVSScriptAPI(int version)
{
    return forward(&script_lib, &script_tried, NULL, "getVSScriptAPI", version);
}

EXPORT const void *getVapourSynthAPI(int version)
{
    return forward(&core_lib, &core_tried, CORE_NAME, "getVapourSynthAPI", version);
}
