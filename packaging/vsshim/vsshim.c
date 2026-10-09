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

#include <stdlib.h>
#include <string.h>

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
#define SEP '/'
#ifdef __APPLE__
#define CORE_NAME "libvapoursynth.dylib"
#else
#define CORE_NAME "libvapoursynth.so"
#endif
#endif

typedef const void *(*api_fn)(int);

/* Loads `name` from the folder of SVP's VSScript library (or that library
 * itself when `name` is NULL), once. */
static lib_t load(const char *name)
{
    const char *script = getenv("EMBYCLIENTPLUS_VSSCRIPT");
    if (!script || !*script)
        return NULL;
    if (!name)
        return open_lib(script);
    const char *slash = strrchr(script, SEP);
    size_t dir = slash ? (size_t)(slash - script + 1) : 0;
    size_t len = dir + strlen(name) + 1;
    char *path = malloc(len);
    if (!path)
        return NULL;
    memcpy(path, script, dir);
    strcpy(path + dir, name);
    lib_t lib = open_lib(path);
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
    return fn ? fn(version) : NULL;
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
