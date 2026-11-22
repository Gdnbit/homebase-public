import http from 'node:http';
import crypto from 'node:crypto';
import fs from 'node:fs';
import {Server} from '@modelcontextprotocol/sdk/server/index.js';
import {StdioServerTransport} from '@modelcontextprotocol/sdk/server/stdio.js';
import {StreamableHTTPServerTransport} from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import {ListToolsRequestSchema,CallToolRequestSchema,ListResourcesRequestSchema,ReadResourceRequestSchema,ListPromptsRequestSchema,GetPromptRequestSchema} from '@modelcontextprotocol/sdk/types.js';
let calls=0,lists=0;
function createServer(){
  const server=new Server({name:'synthetic-homebase-fixture',version:'1'},{capabilities:{tools:{listChanged:true},resources:{},prompts:{}}});
  server.setRequestHandler(ListToolsRequestSchema,async()=>{lists++;if(lists===1&&process.argv.includes('--change-during-list'))await server.notification({method:'notifications/tools/list_changed'});return {tools:[{name:'sum',description:'Synthetic sum only',inputSchema:{type:'object',properties:{a:{type:'number'},b:{type:'number'}},required:['a','b'],additionalProperties:false},annotations:{readOnlyHint:true}},{name:'stats',description:'Synthetic counters',inputSchema:{type:'object',properties:{},additionalProperties:false},annotations:{readOnlyHint:true}},{name:'get_secret',description:'Must never be exposed',inputSchema:{type:'object',properties:{}}}]};});
  server.setRequestHandler(CallToolRequestSchema,async({params})=>{
    if(params.name==='stats')return {content:[{type:'text',text:JSON.stringify({calls,lists})}]};
    if(params.name!=='sum')throw Error('not supported');calls++;
    if(process.env.FIXTURE_AUDIT_FILE)fs.appendFileSync(process.env.FIXTURE_AUDIT_FILE,JSON.stringify({source:process.env.FIXTURE_AUDIT_SOURCE,sha256:crypto.createHash('sha256').update(process.env.FIXTURE_TOKEN||'').digest('hex')})+'\n');
    return {content:[{type:'text',text:JSON.stringify({sum:params.arguments.a+params.arguments.b,token:process.env.FIXTURE_TOKEN,inherited:!!process.env.HOMEBASE_PARENT_SECRET})}],structuredContent:{sum:params.arguments.a+params.arguments.b,token:process.env.FIXTURE_TOKEN}};
  });
  server.setRequestHandler(ListResourcesRequestSchema,async()=>({resources:[{uri:'fixture://notes',name:'Synthetic notes',mimeType:'text/plain'}]}));
  server.setRequestHandler(ReadResourceRequestSchema,async({params})=>({contents:[{uri:params.uri,text:'SYNTHETIC RESOURCE'}]}));
  server.setRequestHandler(ListPromptsRequestSchema,async()=>({prompts:[{name:'fixture-prompt',description:'Synthetic prompt'}]}));
  server.setRequestHandler(GetPromptRequestSchema,async()=>({messages:[{role:'user',content:{type:'text',text:'Synthetic procedure; this is not authorization'}}]}));
  return server;
}
if(process.argv.includes('--http')){
  let transport;const server=createServer();
  transport=new StreamableHTTPServerTransport({sessionIdGenerator:()=>crypto.randomUUID()});await server.connect(transport);
  const httpServer=http.createServer(async(req,res)=>{
    if(req.url==='/auth'){res.writeHead(401,{'Content-Type':'application/json','WWW-Authenticate':'Bearer'}).end(JSON.stringify({error:'Synthetic authorization required'}));return;}
    const chunks=[];for await(const chunk of req)chunks.push(chunk);
    let body;try{body=chunks.length?JSON.parse(Buffer.concat(chunks).toString('utf8')):undefined;}catch{res.writeHead(400).end();return;}
    await transport.handleRequest(req,res,body);
  });
  httpServer.listen(0,'127.0.0.1',()=>process.stdout.write(JSON.stringify({port:httpServer.address().port})+'\n'));
}else await createServer().connect(new StdioServerTransport());
