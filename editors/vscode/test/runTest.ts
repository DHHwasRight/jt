// Starts VS Code with the extension under development and runs suite.ts in it.
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import { runTests } from "@vscode/test-electron";

async function main() {
  const root = path.resolve(__dirname, "..");
  // The installed VS Code, when there is one; otherwise a downloaded copy.
  const installed = "/Applications/Visual Studio Code.app/Contents/MacOS/Code";
  const userData = fs.mkdtempSync(path.join(os.tmpdir(), "jt-vscode-"));
  await runTests({
    vscodeExecutablePath: fs.existsSync(installed) ? installed : undefined,
    extensionDevelopmentPath: root,
    extensionTestsPath: path.join(root, "out", "suite.js"),
    launchArgs: [
      path.join(root, "test", "fixture"),
      "--disable-extensions",
      "--disable-workspace-trust",
      "--skip-welcome",
      "--user-data-dir",
      userData,
    ],
  });
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
