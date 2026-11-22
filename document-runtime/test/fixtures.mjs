// Synthetic PDF bytes, no downloaded or personal material. ToUnicode maps
// include Chinese and a surrogate-pair emoji without embedding system fonts.
export function pdfFixture(pageTexts=['0048 0065 006C 006C 006F'],{encrypted=false,action=false,wide=false}={}){
  const objects=[],add=s=>(objects.push(s),objects.length);
  add('');add('');
  const unicode='begincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Synthetic def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n3 beginbfchar\n<0001> <4E2D>\n<0002> <6587>\n<0003> <D83DDE42>\nendbfchar\n1 beginbfrange\n<0020> <007E> <0020>\nendbfrange\nendcmap';
  const stream=text=>`<< /Length ${Buffer.byteLength(text)} >>\nstream\n${text}\nendstream`;
  const cmap=add(stream(unicode));
  const descendant=add('<< /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 0 >> /DW 1000 >>');
  const font=add(`<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /Identity-H /DescendantFonts [${descendant} 0 R] /ToUnicode ${cmap} 0 R >>`);
  const pages=[];
  for(const text of pageTexts){
    const content=add(stream(text===null?'q 1 0 0 rg 20 20 20 20 re f Q':`BT /F1 12 Tf 40 740 Td <${text.replaceAll(' ','')}> Tj ET`));
    pages.push(add(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${wide?2000000:600} 800] /Resources << /Font << /F1 ${font} 0 R >> >> /Contents ${content} 0 R >>`));
  }
  objects[0]=`<< /Type /Catalog /Pages 2 0 R${action?' /OpenAction << /S /JavaScript /JS (throw new Error(SYNTHETIC_DOCUMENT_ACTION_MUST_NOT_EXECUTE)) >>':''} >>`;
  objects[1]=`<< /Type /Pages /Count ${pages.length} /Kids [${pages.map(i=>`${i} 0 R`).join(' ')}] >>`;
  let encryption='';if(encrypted){const id=add(`<< /Filter /Standard /V 1 /R 2 /O <${'00'.repeat(32)}> /U <${'00'.repeat(32)}> /P -4 >>`);encryption=` /Encrypt ${id} 0 R /ID [<${'11'.repeat(16)}> <${'11'.repeat(16)}>]`;}
  let output='%PDF-1.7\n',offsets=[0];
  objects.forEach((object,n)=>{offsets.push(Buffer.byteLength(output));output+=`${n+1} 0 obj\n${object}\nendobj\n`;});
  const xref=Buffer.byteLength(output);output+=`xref\n0 ${objects.length+1}\n0000000000 65535 f \n`+offsets.slice(1).map(n=>`${String(n).padStart(10,'0')} 00000 n \n`).join('');
  output+=`trailer\n<< /Size ${objects.length+1} /Root 1 0 R${encryption} >>\nstartxref\n${xref}\n%%EOF\n`;return Buffer.from(output);
}
export const request=(bytes,pages)=>({version:1,mime_type:'application/pdf',bytes_base64:bytes.toString('base64'),...(pages?{pages}: {})});
