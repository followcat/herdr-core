// Regenerate the closed label patterns and isolated Ink fixtures. This never
// imports the Command Code entry point. Pass its installed package directory.
import fs from 'node:fs';
import {createHash} from 'node:crypto';
import {createRequire} from 'node:module';
const root=process.argv[2];
if(!root) throw new Error('usage: node scripts/detect/command-code-sweep.mjs <command-code package directory> [--manifest]');
const bundle=fs.readFileSync(`${root}/dist/cli.mjs`,'utf8');
const sha=createHash('sha256').update(bundle).digest('hex');
if(sha!=='1239c20baa8b475ac0d0a1955376ac68570e11a265c071de4146e3ea72623ad9') throw new Error('renderer changed; review the source before updating this generator');
const require=createRequire(`${root}/package.json`);
const {default:React}=await import(require.resolve('react'));
const {renderToString,Box,Text}=await import(require.resolve('ink'));
const {mainSymbols,fallbackSymbols}=await import(require.resolve('figures'));
const h=React.createElement,t=text=>h(Text,null,text);
const sh=text=>h(Text,null,...[...text].map((c,i)=>h(Text,{key:i},c)));
const escape=s=>s.replace(/[.*+?^${}()|[\]\\]/g,'\\$&');
// Uv and provider metadata are read as literals, never evaluated as code.
const verbs=JSON.parse(bundle.match(/Uv=(\[[^\]]+\])/u)[1]);
if(verbs.length!==74) throw new Error('default verb inventory changed');
const providers=[...bundle.slice(bundle.indexOf('ia={'),bundle.indexOf('ia={')+1600).matchAll(/label:"([^"]+)"/g)].map(m=>m[1]);
const labels=verbs.map(text=>({text,pattern:escape(text)}));
const literal=text=>labels.push({text,pattern:escape(text)});
const param=(stem,value)=>labels.push({text:stem+value,pattern:escape(stem)+String.raw`[^\r\n]+`});
for(const family of ['Shell command','Process termination','Tool call','File operation']) for(const result of ['allowed','denied']) literal(`${family} ${result}`);
for(const text of ['Compacting conversation...','Ready...','Answers submitted','Failed to set provider','Restore failed','Connecting to the cloud…','Loading the conversation…','Loading the conversation','Starting the cloud session','Starting the cloud session…','Checking for updates...','Naming session…','Failed to set model','Failed to set theme','Failed to set compact mode','Restoring checkpoint...','Removing share link...','Context limit reached. Auto-compacting the conversation...','Connecting to the cloud','Creating a cloud session…','Restarting the cloud session…','Waiting for your answer…','Waiting for file edit permission...','Waiting for shell command permission...','Waiting for tool permission...','Permission granted','Permission denied','Question cancelled','Insufficient credits','Model not available','Premium credits exhausted','Spend limit reached','Usage limit reached','Error','No active session to unshare','Interrupted · What should Command Code do instead?']) literal(text);
for(const [stem,value] of [
 ['Executing: ','echo renderer'],['Theme set to: ','dark'],['Model set to ','Claude Opus 4.6'],['Provider set to ','OpenAI ChatGPT/Codex'],
 ['Provider authentication ','required. Use /connect to authenticate with OpenAI.'],['Run /connect to ','authenticate with OpenAI first'],
 ['Cloning ','renderer…'],['Updating ','1.88.3 → 1.88.4'],['Language set to: ','English'],
 ['Could not save language: ','permission denied. Report issues at https://cond.dev/issue'],['Compact mode set to: ','auto'],['Switched to ','Claude Sonnet'],['Failed to unshare: ','server unavailable']
]) param(stem,value);
for(const provider of [...providers,'OpenAI']) literal(`${provider} authentication successful`);
labels.push({text:'Connection dropped mid-response. Retrying... (2/3)',pattern:String.raw`Connection dropped mid-response\. Retrying\.\.\. \([0-9]+/[0-9]+\)`});
labels.push({text:'Connection issue. Retrying... (attempt 2)',pattern:String.raw`Connection issue\. Retrying\.\.\. \(attempt [0-9]+\)`});
for(const [stem,value] of [['Setting effort to ','high'],['Switching to ','Claude Opus 4.6']]) labels.push({text:`${stem}${value} in the cloud…`,pattern:escape(stem)+String.raw`[^\r\n]+ in the cloud…`});
const fullRange=process.argv.includes('--all-widths');
const widths=fullRange?Array.from({length:105},(_,i)=>i+16):[18,24,32,38,42,45,60,68,72,80,120];
const pointers=[...new Set([mainSymbols.pointer,fallbackSymbols.pointer])];
if(pointers.join(',')!=='❯,>') throw new Error('figures pointer inventory changed');
const modes=w=>[w>=72?'all':w>=42?'medium':'none',...(w>=38&&w<42?['medium']:w>=68&&w<72?['all']:[])];
function status(text,width,mode,icon) {
 return h(Box,{width,height:1,paddingLeft:0,marginLeft:0},
  h(Box,{flexShrink:0},t(' '),t(`${icon} `),sh(text),t(' ')),
  mode==='medium'&&h(Box,{marginLeft:1,flexShrink:0},t('1.1k')),
  mode==='all'&&h(Box,{columnGap:1,marginLeft:1,flexShrink:0},...['esc','to interrupt','•','6s','•','↓','1.1k'].map(t)));
}
function overlay(width,title,subtitle,children,footer) {
 return h(Box,{flexDirection:'column',width:'100%'},
  t('─'.repeat(width)),h(Box,null,t(title)),
  subtitle?h(Box,{marginBottom:1},t(subtitle)):h(Box,{marginBottom:1}),
  ...children,h(Box,{marginTop:1},t(footer)));
}
const footer='↑/↓ navigate · enter select · Run cmd --yolo to bypass all permissions (Docs ↗)';
function permission(width,pointer,family,risk,selected) {
 const file=['edit','create','delete','execute','operation'].includes(family);
 const title=family==='shell'?'Execute Shell Command':family==='generic'?'Tool Permission':({edit:'Edit File note.txt',create:'Create File',delete:'Delete File',execute:'Execute File',operation:'File Operation'})[family];
 const subtitle=file?null:`Command Code needs to ${family==='shell'?'execute echo renderer':'run renderer_tool'}.`;
 const scoped=family==='shell'?"Yes, don't ask again for `echo` commands in this project":family==='generic'?"Yes, don't ask again for renderer_tool in this project":'Yes, allow all edits this session [shift+tab]';
 const choices=['Yes',...risk?[]:[scoped],'No, tell Command Code what to do differently'];
 const children=[];
 if(risk) children.push(h(Box,{marginBottom:1},t(`${pointer===fallbackSymbols.pointer?fallbackSymbols.warning:mainSymbols.warning} Destructive command. This is never auto-approved.`)));
 if(file) children.push(h(Box,{marginBottom:1},t(`Do you want to ${({edit:'make this edit to',create:'create',delete:'delete',execute:'execute',operation:'modify'})[family]} note.txt?`)));
 children.push(h(Box,{flexDirection:'column'},...choices.map((value,i)=>h(Box,{key:i},t(i===selected?pointer:' '),t(` ${i+1}. ${value}`)))));
 return overlay(width,title,subtitle,children,footer);
}
function trust(width,pointer,selected) {
 return overlay(width,'Do you trust the files in this folder?','/home/user/project',[
  h(Box,{flexDirection:'column'},t('Command Code may read files in this folder. Reading untrusted files may lead Command Code to behave in unexpected ways.'),
   h(Box,{marginTop:1},t('With your permission Command Code may execute files in this folder. Executing untrusted code is unsafe.'))),
  h(Box,{marginTop:1,flexDirection:'column'},...['1. Yes, proceed','2. No, exit'].map((value,i)=>h(Box,{key:i},t(i===selected?pointer:' '),t(` ${value}`))))
 ],'↑/↓ to navigate · enter to select · esc to exit');
}
const render=(element,width)=>renderToString(element,{columns:width});
const rows=[];
for(const width of widths) {
 for(const [i,label] of labels.entries()) for(const mode of modes(width)) rows.push({width,family:'status',label:label.text,mode,body:render(status(label.text,width,mode,['·','○','◇','☆','✧','⌘'][i%6]),width)});
 for(const label of ['Setting effort to high in the cloud…','Switching to Claude Opus 4.6 in the cloud…','Connecting to the cloud…','Creating a cloud session…','Restarting the cloud session…','Starting the cloud session…','Loading the conversation…','Cloning renderer…']) rows.push({width,family:'cloud',label,body:render(h(Box,{marginLeft:1},t('○ '),sh(label)),width)});
 for(let dots=0;dots<=3;dots++) rows.push({width,family:'retry',dots,body:render(h(Box,{marginBottom:1},t('Connection Issue. Retrying (attempt 2)'+'.'.repeat(dots)+' '.repeat(3-dots))),width)});
 for(const pointer of pointers) {
  for(const family of ['shell','edit','create','delete','execute','operation','generic']) for(const risk of [false,true]) for(const selected of [0,risk?1:2]) rows.push({width,family,risk,pointer,selected,body:render(permission(width,pointer,family,risk,selected),width)});
  for(const selected of [0,1]) rows.push({width,family:'trust',pointer,selected,body:render(trust(width,pointer,selected),width)});
  rows.push({width,family:'completed',pointer,body:render(h(Box,{flexDirection:'column'},t('Finished reply'),t('─'.repeat(width)),t(`${pointer} Ask your question...`),t('─'.repeat(width)),t('  ? for shortcuts')),width)});
 }
}
const source={version:'1.88.3',sha256:sha,dependencies:Object.fromEntries(['ink','react','figures'].map(name=>[name,JSON.parse(fs.readFileSync(`${root}/node_modules/${name}/package.json`,'utf8')).version]))};
const file=`crates/herdr-core/tests/fixtures/detect/${fullRange?'cmd-physical-sweep':'cmd-width-sweep'}.json`;
let output;
if(fullRange) {
 const bodies=[],index=new Map();
 const packed=rows.map(row=>{
  if(!index.has(row.body)) {index.set(row.body,bodies.length);bodies.push(row.body);}
  return [row.width,row.family,row.pointer??null,index.get(row.body)];
 });
 output=JSON.stringify({source,widths,pointers,bodies,rows:packed})+'\n';
} else output=JSON.stringify({source,widths,pointers,labels,rows},null,2)+'\n';
fs.writeFileSync(file,output);
console.log(JSON.stringify({labels:labels.length,rows:rows.length,bytes:Buffer.byteLength(output)}));
if(process.argv.includes('--manifest')) {
 const file='crates/herdr-core/src/detect/manifests/command-code.toml';
 let source=fs.readFileSync(file,'utf8');
 const suffix=String.raw`(?:[ \t]+(?:\d+(?:\.\d+)?k?|esc to interrupt[^\r\n]*))?(?:\r?\n[ \t]*(?:esc to interrupt[^\r\n]*|•[ \t]+\d[^\r\n]*|\d+(?:\.\d+)?k?))?(?:\r?\n[ \t]*⎿[ \t]+Tip:\s+Use\s+shift\+tab\s+to\s+enable\s+accept\s+edits)*(?:\r?\n {3}Large\s+repo:\s+it\s+is\s+cloned\s+once\s+per\s+session,\s+and\s+reopening\s+is\s+fast\.)?\s*\z`;
 const shape=String.raw`(?m)^ ?[·☆⌘◇○✧][ \t]*(?P<prefix>\S[^\r\n]*?)`+suffix;
 const retry=String.raw`(?m)^(?P<prefix>\S[^\r\n]*(?:[ \t]*\r?\n ?\S[^\r\n]*)*)[ \t]*\s*\z`;
 const patterns=[];let group=[];
 for(const {pattern} of labels) {
  if([...`(?:${[...group,pattern].join('|')})`].length>512) {patterns.push(`(?:${group.join('|')})`);group=[];}
  group.push(pattern);
 }
 if(group.length) patterns.push(`(?:${group.join('|')})`);
 const quote=s=>`'${s}'`;
 const spec=(regex,patterns)=>`{ prefix = { regex = ${quote(regex)}, min_chars = 6, patterns = [${patterns.map(quote).join(', ')}] } }`;
 const w=String.raw`(?:[ \t]+|\r?\n {2,3})`;
 const words=s=>s.split(' ').map(escape).join(w);
 const value=String.raw`\S[^\r\n]*(?:\r?\n {2,3}\S[^\r\n]*)*`;
 const hint=String.raw`(?:\r?\n {3}Large\s+repo:\s+it\s+is\s+cloned\s+once\s+per\s+session,\s+and\s+reopening\s+is\s+fast\.)?`;
 const cloudLabels=[...['Connecting to the cloud…','Creating a cloud session…','Restarting the cloud session…','Starting the cloud session…','Loading the conversation…'].map(words),
  words('Cloning')+w+value,...['Setting effort to','Switching to'].map(stem=>words(stem)+w+value+w+words('in the cloud…'))];
 const cloud=cloudLabels.map(label=>`{ regex = [${quote(String.raw`(?m)^ ?[·☆⌘◇○✧][ \t]*`+label+hint+String.raw`\s*\z`)}] }`);
 const entries=[spec(shape,patterns),spec(retry,[String.raw`Connection Issue\. Retrying \(attempt [0-9]+\)\.{0,3}`]),...cloud];
 let a=source.indexOf('any = [',source.indexOf('id = "turn_footer"')),b=source.indexOf('\n]\n',a)+2;
 source=source.slice(0,a)+'any = [\n'+entries.map(e=>'  '+e+',\n').join('')+']'+source.slice(b);
 a=source.indexOf('  { region = "above_full_width_prompt_box",',source.indexOf('id = "composer_prompt"'));b=source.indexOf('\n  ] },',a)+7;
 source=source.slice(0,a)+'  { region = "above_full_width_prompt_box", logical_lines = true, any = [\n'+entries.map(e=>'    '+e+',\n').join('')+'  ] },'+source.slice(b);
 source=source.replace('min_engine_version = 7','min_engine_version = 8').replace('2026.11.05.14','2026.11.05.15');
 source=source.replace('region = \"above_full_width_prompt_box\"\nvisible_working', 'region = \"above_full_width_prompt_box\"\nlogical_lines = true\nvisible_working');
 fs.writeFileSync(file,source);
 console.log(JSON.stringify({patterns:patterns.length,shape_chars:[...shape].length,manifest_bytes:Buffer.byteLength(source)}));
}
