import { build } from "vite";
import path from "node:path";
import { fileURLToPath } from "node:url";
const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),"..");
await build({configFile:false,root,logLevel:"warn",define:{"import.meta.url":"location.href"},build:{target:"es2022",outDir:path.join(root,".cache/html-viewer"),emptyOutDir:true,lib:{entry:path.join(root,"html-viewer/viewer.ts"),name:"OOOSplatOffline",formats:["iife"],fileName:()=>"runtime.js"},minify:true}});
