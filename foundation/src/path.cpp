#include "cpg/path.h"
#include "cpg/error.h"
#include "cpg/text.h"
#include "cpg/win_handle.h"

#include <algorithm>
#include <cwctype>

namespace cpg {
namespace {

std::wstring cleanForwardPath(const std::wstring &path)
{
    // QDir::cleanPath equivalent over the already backslash-folded form:
    // resolve "." and "..", drop duplicate separators and trailing slashes,
    // never popping above a root (drive, UNC share or leading slash).
    if (path.empty()) return {};
    std::size_t index = 0;
    while (index < path.size() && path[index] == L'/') ++index;
    const std::size_t leadingSlashes = index;
    const bool rooted = leadingSlashes > 0;
    std::vector<std::wstring> raw;
    std::size_t start = index;
    for (std::size_t position = index; position <= path.size(); ++position) {
        if (position == path.size() || path[position] == L'/') {
            if (position > start) raw.emplace_back(path.substr(start, position - start));
            start = position + 1;
        }
    }
    std::vector<std::wstring> kept;
    // The drive letter (or the \\server\share pair) is an unpopable root.
    const bool driveRoot = !raw.empty() && raw.front().size() == 2 && raw.front()[1] == L':';
    const std::size_t rootSize = leadingSlashes >= 2 ? 2 : (driveRoot ? 1 : 0);
    for (const std::wstring &segment : raw) {
        if (segment == L".") continue;
        if (segment == L"..") {
            if (kept.size() > rootSize && kept.back() != L"..") { kept.pop_back(); continue; }
            // At or above a root the segment is dropped; in a relative path a
            // leading ".." is preserved.
            if (rooted || (rootSize > 0 && kept.size() >= rootSize)) continue;
            kept.push_back(segment);
            continue;
        }
        kept.push_back(segment);
    }
    std::wstring result;
    if (leadingSlashes >= 2) result = L"//";
    else if (leadingSlashes == 1) result = L"/";
    for (const std::wstring &segment : kept) {
        if (!result.empty() && result.back() != L'/') result.push_back(L'/');
        result.append(segment);
    }
    if (result.empty()) result = rooted ? (leadingSlashes >= 2 ? L"//" : L"/") : L".";
    return result;
}

std::wstring foldCase(const std::wstring &text)
{
    if (text.empty()) return text;
    const int size = LCMapStringW(LOCALE_INVARIANT, LCMAP_LOWERCASE, text.c_str(),
                                  static_cast<int>(text.size()), nullptr, 0);
    if (size <= 0) return text;
    std::wstring folded(static_cast<std::size_t>(size), L'\0');
    LCMapStringW(LOCALE_INVARIANT, LCMAP_LOWERCASE, text.c_str(),
                 static_cast<int>(text.size()), folded.data(), size);
    return folded;
}

} // namespace

std::filesystem::path knownFolder(REFKNOWNFOLDERID id)
{
    PWSTR result = nullptr;
    if (FAILED(SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY, nullptr, &result)))
        throw Error("CONFIG_PATH_UNAVAILABLE", "Cannot resolve the user directory.");
    std::filesystem::path path{std::wstring(result)};
    CoTaskMemFree(result);
    return path;
}

std::wstring normalizedWindowsPath(std::wstring_view path)
{
    std::wstring folded(path);
    for (wchar_t &value : folded) if (value == L'\\') value = L'/';
    const wchar_t uncPrefix[] = L"//?/UNC/";
    const wchar_t devicePrefix[] = L"//?/";
    std::wstring_view view(folded);
    if (view.size() > 8 && _wcsnicmp(view.data(), uncPrefix, 8) == 0) {
        view = view.substr(8);
        folded = L"//" + std::wstring(view);
    } else if (view.size() > 4 && _wcsnicmp(view.data(), devicePrefix, 4) == 0) {
        view = view.substr(4);
        folded = std::wstring(view);
    }
    return foldCase(cleanForwardPath(folded));
}

bool pathWithin(const std::filesystem::path &root, const std::filesystem::path &child)
{
    const std::wstring normalizedRoot = normalizedWindowsPath(root.wstring());
    const std::wstring normalizedChild = normalizedWindowsPath(child.wstring());
    return normalizedRoot == normalizedChild
        || (normalizedChild.size() > normalizedRoot.size()
            && normalizedChild.compare(normalizedRoot.size(), 1, L"/") == 0
            && normalizedChild.compare(0, normalizedRoot.size(), normalizedRoot) == 0);
}

std::optional<std::filesystem::path> canonicalPathOf(const std::filesystem::path &path,
                                                     bool directory)
{
    UniqueHandle handle(CreateFileW(path.c_str(), FILE_READ_ATTRIBUTES,
                                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                                    nullptr, OPEN_EXISTING,
                                    FILE_FLAG_BACKUP_SEMANTICS, nullptr));
    if (!handle.valid()) return std::nullopt;
    if (directory) {
        BY_HANDLE_FILE_INFORMATION information{};
        if (!GetFileInformationByHandle(handle.get(), &information)
            || (information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) == 0)
            return std::nullopt;
    } else {
        BY_HANDLE_FILE_INFORMATION information{};
        if (!GetFileInformationByHandle(handle.get(), &information)
            || (information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0
            || (information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0)
            return std::nullopt;
    }
    std::wstring finalPath;
    DWORD size = GetFinalPathNameByHandleW(handle.get(), nullptr, 0, FILE_NAME_NORMALIZED);
    if (size == 0) return std::nullopt;
    finalPath.resize(size);
    size = GetFinalPathNameByHandleW(handle.get(), finalPath.data(),
                                     static_cast<DWORD>(finalPath.size()),
                                     FILE_NAME_NORMALIZED);
    if (size == 0 || size >= finalPath.size()) return std::nullopt;
    finalPath.resize(size);
    const wchar_t devicePrefix[] = L"\\\\?\\UNC\\";
    if (finalPath.rfind(devicePrefix, 0) == 0) finalPath = L"\\\\" + finalPath.substr(8);
    else if (finalPath.rfind(L"\\\\?\\", 0) == 0) finalPath = finalPath.substr(4);
    return std::filesystem::path(finalPath);
}

} // namespace cpg
