#!/usr/bin/env python3
"""Collect pinned Cargo source materials; check without regenerating product inputs."""
import argparse,hashlib,json,pathlib,platform,re,stat,subprocess,shutil,tomllib
ROOT=pathlib.Path(__file__).resolve().parents[1]
DEST=ROOT/'resources/third-party'
LICENSE_SOURCES=ROOT/'resources/license-sources'
# These published archives omit the repository's licensing declaration and full
# standard terms. Pin both their exact upstream commits and the reviewed texts;
# the generated manifest is never the authority for supplemental materials.
OBJC2_REPOSITORY='https://github.com/madsmtm/objc2'
OBJC2_FRAMEWORK_COMMIT='7b1abfd750a2cacaea71d6a56ecfb83cb7de560b'
OBJC2_PINNED={
 'block2':('0.6.2','b4167b582b2f75f9a1be75495c41b765344fd03c'),
 'dispatch2':('0.3.1','8852b424193ca41602281b3d7540d7c8ed51e49a'),
 'objc2':('0.6.5','d7d2fa23ceaa5e6096c923b081040e5d81b3b9df'),
 'objc2-encode':('4.1.0','8d214f5477365ffcbcbb7de058c86ed9a518efb7'),
 **{name:('0.3.2',OBJC2_FRAMEWORK_COMMIT) for name in [
  'objc2-app-kit','objc2-audio-toolbox','objc2-av-foundation','objc2-avf-audio',
  'objc2-cloud-kit','objc2-core-audio','objc2-core-audio-types','objc2-core-data',
  'objc2-core-foundation','objc2-core-graphics','objc2-core-image','objc2-core-media',
  'objc2-core-text','objc2-core-video','objc2-foundation','objc2-image-io',
  'objc2-io-surface','objc2-media-toolbox','objc2-quartz-core','objc2-speech']},
}
OBJC2_DECLARATION_SHA256='7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54'
SPDX_TERMS={
 'MIT':'6d7c9e9d7d12c8d3753bdb24417b84819ba4d6c2f025c37948113d86dc81eef9',
 'Apache-2.0':'074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff',
 'Zlib':'bfb1112d49db5b1daecdfef24bd7e2f3ea0bafb33aa67aa0ab51e2bf8407c03d',
}
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def supplemental_materials(package,source,relative,write):
 if package['repository']!=OBJC2_REPOSITORY:return []
 version,commit=OBJC2_PINNED[package['name']]
 assert package['version']==version,'unreviewed objc2 version'
 assert json.loads((source/'.cargo_vcs_info.json').read_text())['git']['sha1']==commit,'unexpected objc2 upstream commit'
 declared=package['license']
 assert declared in ['MIT','Zlib OR Apache-2.0 OR MIT'],'unreviewed objc2 terms'
 inputs=[(LICENSE_SOURCES/'objc2'/commit/'LICENSE.md','supplemental/upstream-LICENSE.md',OBJC2_DECLARATION_SHA256,
          'https://raw.githubusercontent.com/madsmtm/objc2/'+commit+'/LICENSE.md','upstream licensing declaration')]
 for license in (['MIT'] if declared=='MIT' else ['MIT','Apache-2.0','Zlib']):
  inputs.append((LICENSE_SOURCES/'spdx-3.26.0'/(license+'.txt'),'supplemental/'+license+'.txt',SPDX_TERMS[license],
                 'https://raw.githubusercontent.com/spdx/license-list-data/v3.26.0/text/'+license+'.txt','standard terms; upstream copyright placeholders are not filled or invented'))
 rows=[]
 for file,name,digest,url,kind in inputs:
  assert stat.S_ISREG(file.lstat().st_mode),'supplemental license must be a regular file'
  parent=file.parent
  while True:
   assert stat.S_ISDIR(parent.lstat().st_mode),'supplemental license has linked/non-directory ancestor'
   if parent==LICENSE_SOURCES.parent:break
   assert parent!=parent.parent,'supplemental license escaped source root'
   parent=parent.parent
  assert sha(file)==digest,'supplemental license differs from reviewed pinned text'
  out=DEST/relative/name
  if write:out.parent.mkdir(parents=True,exist_ok=True);out.write_bytes(file.read_bytes())
  rows.append({'path':str(out.relative_to(DEST)),'source_relative':'resources/license-sources/'+str(file.relative_to(LICENSE_SOURCES)),
               'sha256':digest,'bytes':file.stat().st_size,'origin':{'url':url,'kind':kind,'crate_upstream_commit':commit}})
 # Retain the archive's own authors/declaration and commit evidence, including
 # for packages that supplied no attribution-bearing source files at all.
 for name in ['Cargo.toml.orig','.cargo_vcs_info.json']:
  file=source/name;out=DEST/relative/name
  if write:out.parent.mkdir(parents=True,exist_ok=True);out.write_bytes(file.read_bytes())
  rows.append({'path':str(out.relative_to(DEST)),'source_relative':name,'sha256':sha(file),'bytes':file.stat().st_size})
 return rows
def metadata():
 host=subprocess.check_output(['rustc','-vV'],text=True).split('host: ')[1].splitlines()[0]
 return host,json.loads(subprocess.check_output(['cargo','metadata','--offline','--locked','--filter-platform',host,'--format-version','1'],cwd=ROOT))
def generate(write=True):
 host,m=metadata();assert host=='aarch64-apple-darwin','Only this development target is reviewed'
 packages={p['id']:p for p in m['packages']};nodes={n['id']:n for n in m['resolve']['nodes']};scopes={};queue=[(i,'runtime') for i in m['workspace_members']]
 while queue:
  id,kind=queue.pop();p=packages[id]
  if any('proc-macro' in t['kind'] for t in p['targets']):kind='build'
  if kind in scopes.setdefault(id,set()):continue
  scopes[id].add(kind)
  for dep in nodes[id]['deps']:
   for edge in dep['dep_kinds']:
    next_kind=kind if kind!='runtime' else ('build' if edge['kind']=='build' else 'dev' if edge['kind']=='dev' else 'runtime')
    queue.append((dep['pkg'],next_kind))

 if write:DEST.mkdir(parents=True,exist_ok=True)
 rows=[]
 for id,p in sorted(packages.items(),key=lambda kv:(kv[1]['name'],kv[1]['version'])):
  if p['source'] is None:continue
  source=pathlib.Path(p['manifest_path']).parent;relative=pathlib.Path('crates')/(p['name']+'-'+p['version']);files=[]
  for file in sorted(source.rglob('*')):
   if not file.is_file() or file.is_symlink():continue
   name=file.name.lower();data=file.read_bytes()
   notice=p['license']=='MPL-2.0' or any(x in name for x in ['license','copying','notice','copyright','authors'])
   # Include complete attribution-bearing source files too: ring explicitly
   # assigns some embedded components' terms in individual source headers.
   if not notice:
    lowered=data.lower()
    notice=any(token in lowered for token in [b'copyright',b'spdx-license-identifier',b'permission is hereby granted',b'redistribution and use'])
   if not notice:continue
   out=DEST/relative/file.relative_to(source)
   if write:out.parent.mkdir(parents=True,exist_ok=True);out.write_bytes(data)
   files.append({'path':str(out.relative_to(DEST)),'source_relative':str(file.relative_to(source)),'sha256':hashlib.sha256(data).hexdigest(),'bytes':len(data)})
  files=sorted({f['path']:f for f in [*files,*supplemental_materials(p,source,relative,write)]}.values(),key=lambda f:f['path'])
  assert files,(p['name'],'missing materials')
  license=p['license'];assert license,(p['name'],'unknown SPDX declaration')
  if license == '(MIT OR Apache-2.0) AND Unicode-3.0': selected='MIT AND Unicode-3.0'
  elif ' AND ' in license: selected=license
  elif 'MIT' in license: selected='MIT'
  elif 'Apache-2.0' in license: selected='Apache-2.0'
  else: selected=license
  assert selected in ['MIT','Apache-2.0','MIT AND Unicode-3.0','Apache-2.0 AND ISC','ISC','BSD-3-Clause','MPL-2.0','Unicode-3.0','CDLA-Permissive-2.0','Zlib'], ('unreviewed terms',selected)
  # Retain every supplied alternative in full. This is not an election of a
  # project license or a legal approval of external redistribution.
  rows.append({'name':p['name'],'version':p['version'],'source':p['source'],'package_id':id,'declared_spdx':license,'selection':selected, 'selection_basis':'Where OR options exist choose MIT, else Apache-2.0; AND terms retained together. Embedded components retain their own full notices regardless of crate primary expression. All supplied alternatives remain available. MPL package full source bundled. This is material selection for local development, not legal approval or a project-license election.','scope':sorted(scopes.get(id,{'other-platform-or-lock-only'})),'enabled_features':nodes.get(id,{}).get('features',[]),'source_manifest':'cargo-registry/'+p['name']+'-'+p['version']+'/Cargo.toml','materials':files})
 lock=tomllib.loads((ROOT/'Cargo.lock').read_text())
 known={(p['name'],p['version']) for p in rows}
 lock_only=[p for p in lock['package'] if p.get('source') and (p['name'],p['version']) not in known]
 build_inputs={str(p.relative_to(ROOT)):sha(p) for p in [ROOT/'Cargo.toml',ROOT/'Cargo.lock',*sorted((ROOT/'crates').glob('*/Cargo.toml'))]}
 manifest={'lock_only_packages':lock_only,'lock_only_policy':'Not in this native target metadata graph; not labelled shipped or linked. Cargo.lock records public source/checksum. These excluded-platform packages are not included as runtime license material.','build_inputs':build_inputs,'schema':1,'target':host,'cargo_lock_sha256':sha(ROOT/'Cargo.lock'),'policy':'Overinclusive source materials. runtime means reachable normal target dependency, NOT proof every object is linked; build includes proc macros/build tools; dev and other-platform/lock-only are separately labelled. Complete notice-bearing source files are supplementary attribution material, not build inputs.','packages':rows}
 if write:
  (DEST/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
  print('Collected',len(rows),'packages',sum(len(r['materials']) for r in rows),'materials')
 return manifest
def check(directory=DEST):
 expected_manifest=generate(False)
 source_manifest=json.loads((DEST/'manifest.json').read_text())
 assert source_manifest==expected_manifest,'source license manifest differs from locked native graph/reviewed selections/cached source materials'
 m=json.loads((directory/'manifest.json').read_text());assert m==source_manifest,'installed license manifest differs from trusted source'
 assert all(sha(ROOT/p)==h for p,h in m['build_inputs'].items()),'Cargo manifest/feature drift';assert m['cargo_lock_sha256']==sha(ROOT/'Cargo.lock'),'lock drift';assert m['target']=='aarch64-apple-darwin'
 expected={'manifest.json'}
 for p in m['packages']:
  assert p['declared_spdx'] and p['materials'] and p['scope']
  for f in p['materials']:
   path=pathlib.Path(f['path']);assert not path.is_absolute() and '..' not in path.parts
   q=directory/path;assert not q.is_symlink() and q.is_file();assert sha(q)==f['sha256'] and q.stat().st_size==f['bytes'];expected.add(str(path))
 actual={str(p.relative_to(directory)) for p in directory.rglob('*') if p.is_file()};assert actual==expected,'missing or unexpected material'
 return m
if __name__=='__main__':
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--generate',action='store_true');parser.add_argument('--directory',type=pathlib.Path,default=DEST);a=parser.parse_args()
 if a.generate:generate()
 check(a.directory);print('License materials verified')
