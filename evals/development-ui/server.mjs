import { build, preview } from 'vite';
import react from '@vitejs/plugin-react';
import { fileURLToPath } from 'node:url';
const root=fileURLToPath(new URL('../../',import.meta.url));
const config={configFile:false,envDir:false,root,plugins:[react()],resolve:{alias:{'@':root+'src'}},build:{outDir:(process.env.HOMEBASE_DEVELOPMENT_UI_ROOT||'.tmp/development-ui')+'/ui-build',emptyOutDir:true,rollupOptions:{input:root+'evals/development-ui/index.html'}},preview:{host:'127.0.0.1',port:1437,strictPort:true}};
await build(config);await preview(config);
