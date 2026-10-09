/*
 * "Emby Client+.exe" at the top of the Windows release folder: starts
 * bin\embyclientplus.exe with the same arguments, so people don't have to
 * look inside bin\. GTK needs the real exe to stay in bin\ next to its DLLs.
 *
 * Build: gcc -O2 -municode -mwindows launcher.c icon.o -o "Emby Client+.exe"
 * (packaging/package-windows.sh).
 */

#include <windows.h>
#include <wchar.h>

/* The command line after the program name, quoting kept intact. */
static const wchar_t *arguments(void)
{
    const wchar_t *line = GetCommandLineW();
    int quoted = 0;
    for (; *line; line++) {
        if (*line == L'"')
            quoted = !quoted;
        else if (*line == L' ' && !quoted)
            break;
    }
    while (*line == L' ')
        line++;
    return line;
}

int WINAPI wWinMain(HINSTANCE instance, HINSTANCE previous, PWSTR args, int show)
{
    (void)instance;
    (void)previous;
    (void)args;
    (void)show;
    wchar_t dir[MAX_PATH];
    DWORD len = GetModuleFileNameW(NULL, dir, MAX_PATH);
    if (len == 0 || len >= MAX_PATH)
        return 1;
    wchar_t *slash = wcsrchr(dir, L'\\');
    if (slash)
        *slash = L'\0';

    wchar_t exe[MAX_PATH + 32];
    swprintf(exe, sizeof exe / sizeof *exe, L"%ls\\bin\\embyclientplus.exe", dir);
    const wchar_t *rest = arguments();
    size_t size = wcslen(exe) + wcslen(rest) + 4;
    wchar_t *command = HeapAlloc(GetProcessHeap(), 0, size * sizeof *command);
    if (!command)
        return 1;
    swprintf(command, size, L"\"%ls\" %ls", exe, rest);

    STARTUPINFOW startup = {.cb = sizeof startup};
    PROCESS_INFORMATION process;
    if (!CreateProcessW(exe, command, NULL, NULL, FALSE, 0, NULL, NULL, &startup, &process)) {
        MessageBoxW(NULL, L"Couldn't start bin\\embyclientplus.exe.", L"Emby Client+",
                    MB_ICONERROR);
        return 1;
    }
    CloseHandle(process.hThread);
    CloseHandle(process.hProcess);
    return 0;
}
