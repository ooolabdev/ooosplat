import fs from "node:fs";
import path from "node:path";
const root = path.resolve(process.argv[2]);
// Only dereference file aliases in an explicitly supplied build-stage tree.
function walk(directory) {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) walk(file);
    else if (entry.isSymbolicLink()) {
      if (!fs.statSync(file).isFile()) throw new Error(`Unexpected directory link: ${file}`);
      const bytes = fs.readFileSync(file);
      const mode = fs.statSync(file).mode;
      fs.unlinkSync(file);
      fs.writeFileSync(file, bytes, { mode });
    }
  }
}
walk(root);
