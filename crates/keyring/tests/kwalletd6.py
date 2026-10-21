"""A python-dbusmock template for kwalletd6: the part of org.kde.KWallet that Vinculum uses,
with passwords kept in memory. The wallet opens without a prompt and always has handle 7."""

BUS_NAME = "org.kde.kwalletd6"
MAIN_OBJ = "/modules/kwalletd6"
MAIN_IFACE = "org.kde.KWallet"
SYSTEM_BUS = False


def load(mock, _parameters):
    mock.folders = {}
    mock.AddMethods(
        MAIN_IFACE,
        [
            ("networkWallet", "", "s", 'ret = "kdewallet"'),
            ("open", "sxs", "i", "ret = 7"),
            ("hasFolder", "iss", "b", "ret = args[1] in self.folders"),
            ("createFolder", "iss", "b", "self.folders.setdefault(args[1], {}); ret = True"),
            ("writePassword", "issss", "i", "self.folders.setdefault(args[1], {})[args[2]] = args[3]; ret = 0"),
            ("readPassword", "isss", "s", 'ret = self.folders.get(args[1], {}).get(args[2], "")'),
            ("entryList", "iss", "as", "ret = list(self.folders.get(args[1], {}))"),
            ("removeEntry", "isss", "i", "self.folders.get(args[1], {}).pop(args[2], None); ret = 0"),
        ],
    )
