#include "cli.h"
#include "tui_app.h"
#include <iostream>
#include <string>
#include <vector>

// The CRT provides Unicode argv without a fallible second command-line parse.
int wmain(int argc, wchar_t *argv[])
{
    try {
        std::vector<std::wstring> arguments;
        for (int index = 1; index < argc; ++index) arguments.emplace_back(argv[index]);
        cpg::setInteractiveConsoleEntry(&cpg::tui::runTui);
        return cpg::runCli(arguments);
    } catch (...) {
        std::cerr << "CLI_INVALID: Could not read the Unicode command line.\n";
        return 1;
    }
}
