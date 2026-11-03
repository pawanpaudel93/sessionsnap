// sessionsnap: records the flags this Pi was started with in its session, as a custom entry named
// "sessionsnap.launch", so `sessionsnap restore` resumes the session with them. Pi renames its
// process, which hides its arguments from every other program. The API key is never recorded.
// Written by `sessionsnap install-pi`; `sessionsnap uninstall` removes it.
export default function (pi: any) {
  pi.on("session_start", async () => {
    const argv: string[] = process.argv.slice(2);
    const args: string[] = [];
    for (let i = 0; i < argv.length; i++) {
      const a = argv[i];
      if (a === "--api-key") {
        i++;
        continue;
      }
      if (a.startsWith("--api-key=")) {
        continue;
      }
      args.push(a);
    }
    pi.appendEntry("sessionsnap.launch", { args });
  });
}
