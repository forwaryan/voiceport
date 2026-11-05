import importlib.util,json,pathlib,sys,tempfile,unittest,shutil,os,subprocess
ROOT=pathlib.Path(__file__).resolve().parents[2];sys.path.insert(0,str(ROOT/'scripts'))
from product_inventory import inventory,FILES,FOLDERS
from license_materials import check
class PackagingTests(unittest.TestCase):
 def test_inventory_detects_cargo_docs_untracked_permissions_and_removal(self):
  with tempfile.TemporaryDirectory() as t:
   r=pathlib.Path(t)
   for f in FILES:(r/f).write_text('initial')
   for f in FOLDERS:(r/f).mkdir()
   baseline=inventory(r)
   for name in ['Cargo.lock','README.md','docs/new-untracked.md','crates/new.rs']:
    p=r/name;original=p.read_bytes() if p.exists() else None;p.write_text('changed');self.assertNotEqual(inventory(r),baseline)
    if original is None:p.unlink()
    else:p.write_bytes(original)
   p=r/'Cargo.toml';p.chmod(0o755);self.assertNotEqual(inventory(r),baseline);p.chmod(0o644)
   p.unlink()
   with self.assertRaises(ValueError):inventory(r)
 def test_missing_or_tampered_license_material_is_rejected(self):
  with tempfile.TemporaryDirectory() as t:
   d=pathlib.Path(t)/'materials';shutil.copytree(ROOT/'resources/third-party',d)
   m=check(d);p=d/m['packages'][0]['materials'][0]['path'];data=p.read_bytes();p.write_bytes(b'tampered')
   with self.assertRaises(AssertionError):check(d)
   p.write_bytes(data);p.unlink()
   with self.assertRaises(AssertionError):check(d)
 def test_extra_unlisted_resource_is_rejected(self):
  with tempfile.TemporaryDirectory() as t:
   d=pathlib.Path(t)/'materials';shutil.copytree(ROOT/'resources/third-party',d);(d/'unlisted').write_text('extra')
   with self.assertRaises(AssertionError):check(d)
 def test_missing_helper_rejected(self):
  from check_bundle import validate
  with tempfile.TemporaryDirectory() as t:
   with self.assertRaises(AssertionError):validate(pathlib.Path(t),'missing','missing')
 def test_real_bundle_rejects_changed_helper_and_missing_material(self):
  from check_bundle import validate
  app=ROOT/'dist/VoicePort.app'
  self.assertTrue(app.exists(),'run normal package first')
  target=pathlib.Path(json.loads(subprocess.check_output(['cargo','metadata','--offline','--locked','--no-deps','--format-version','1'],cwd=ROOT))['target_directory'])/'release/voiceport-rs'
  with tempfile.TemporaryDirectory() as t:
   copy=pathlib.Path(t)/'VoicePort.app';shutil.copytree(app,copy)
   helper=copy/'Contents/Helpers/voiceport-rs';helper.chmod(0o644)
   with self.assertRaises(AssertionError):validate(copy,target,pathlib.Path(subprocess.check_output(['swift','build','-c','release','--show-bin-path'],cwd=ROOT,text=True).strip())/'VoicePort')
   helper.chmod(0o755);original=helper.read_bytes();helper.write_bytes(original+b'wrong helper')
   with self.assertRaises((AssertionError,subprocess.CalledProcessError)):validate(copy,target,pathlib.Path(subprocess.check_output(['swift','build','-c','release','--show-bin-path'],cwd=ROOT,text=True).strip())/'VoicePort')
   helper.write_bytes(original)
   (copy/'Contents/Resources/third-party/manifest.json').unlink()
   with self.assertRaises(FileNotFoundError):validate(copy,target,pathlib.Path(subprocess.check_output(['swift','build','-c','release','--show-bin-path'],cwd=ROOT,text=True).strip())/'VoicePort')
 def test_skip_build_metadata_directory_and_missing_product(self):
  from check_bundle import validate
  target=pathlib.Path(json.loads(subprocess.check_output(['cargo','metadata','--offline','--locked','--no-deps','--format-version','1'],cwd=ROOT))['target_directory'])/'release/voiceport-rs'
  with tempfile.TemporaryDirectory() as t:
   directory=pathlib.Path(t)/'alternate-target';directory.mkdir()
   environment=dict(os.environ,CARGO_TARGET_DIR=str(directory),CARGO_NET_OFFLINE='true')
   command=['bash',str(ROOT/'scripts/package_app.sh'),'--skip-build']
   def run():
    result=subprocess.run(command,cwd=t,env=environment,capture_output=True)
    print(json.dumps({'argv':command,'cwd':t,'CARGO_TARGET_DIR':str(directory),'exit':result.returncode}),file=sys.stderr)
    sys.stderr.write(result.stdout.decode(errors='replace')+result.stderr.decode(errors='replace'))
    return result
   missing=run();self.assertEqual(missing.returncode,1);self.assertIn(b'Both Swift and Rust release products are required',missing.stderr)
   (directory/'release').mkdir();copied=directory/'release/voiceport-rs';shutil.copy2(target,copied)
   self.assertEqual(run().returncode,0);self.assertTrue(validate(ROOT/'dist/VoicePort.app',copied,pathlib.Path(subprocess.check_output(['swift','build','-c','release','--show-bin-path'],cwd=ROOT,text=True).strip())/'VoicePort')['signed_copy_matches'])
 def test_materials_include_ring_embedded_and_mpl_source(self):
  m=check();ring=next(p for p in m['packages'] if p['name']=='ring');files=[f['source_relative'] for f in ring['materials']]
  for name in ['LICENSE-other-bits','LICENSE-BoringSSL','third_party/fiat/LICENSE','src/polyfill/once_cell/LICENSE-MIT']:self.assertIn(name,files)
  mpl=[p for p in m['packages'] if p['declared_spdx']=='MPL-2.0'];self.assertTrue(mpl)
  for p in mpl:self.assertTrue(any(f['source_relative'].endswith('.rs') for f in p['materials']))
if __name__=='__main__':unittest.main()

class RevisionBoundaries(unittest.TestCase):
 def make_tree(self,r):
  for f in FILES:(r/f).write_text('owned synthetic input')
  for f in FOLDERS:(r/f).mkdir()
 def test_root_links_types_missing_and_directory_modes(self):
  with tempfile.TemporaryDirectory() as t:
   r=pathlib.Path(t);self.make_tree(r);base=inventory(r)
   (r/'docs').chmod(0o700);self.assertNotEqual(base,inventory(r));(r/'docs').chmod(0o755)
   (r/'docs/empty').mkdir();self.assertNotEqual(base,inventory(r));(r/'docs/empty').rmdir()
   held=r/'held';(r/'docs').rename(held);(r/'docs').symlink_to(held,target_is_directory=True)
   with self.assertRaises(ValueError):inventory(r)
   (r/'docs').unlink();held.rename(r/'docs');(r/'Cargo.toml').unlink();(r/'Cargo.toml').symlink_to(r/'Cargo.lock')
   with self.assertRaises(ValueError):inventory(r)
   (r/'Cargo.toml').unlink();os.mkfifo(r/'Cargo.toml')
   with self.assertRaises(ValueError):inventory(r)
   (r/'Cargo.toml').unlink();(r/'Cargo.toml').write_text('owned');shutil.rmtree(r/'docs')
   with self.assertRaises(ValueError):inventory(r)
 def test_source_manifest_is_not_self_authoritative(self):
  import license_materials as lm
  from unittest.mock import patch
  mutations=[lambda m:m['packages'][0].update(declared_spdx='UNKNOWN'),lambda m:m['packages'][0].update(selection='UNKNOWN'),lambda m:m['packages'].pop(0),lambda m:m['packages'].append(m['packages'][0]),lambda m:m.update(build_inputs={}),lambda m:m['packages'][0].update(enabled_features=['invented']),lambda m:m['packages'][0].update(scope=['invented'])]
  for mutation in mutations:
   with tempfile.TemporaryDirectory() as t:
    d=pathlib.Path(t)/'materials';shutil.copytree(ROOT/'resources/third-party',d);m=json.loads((d/'manifest.json').read_text());mutation(m);(d/'manifest.json').write_text(json.dumps(m))
    with patch.object(lm,'DEST',d):
     with self.assertRaisesRegex(AssertionError,'source license manifest'):lm.check(d)
 def test_signed_wrong_main_and_semantic_license_corruption(self):
  from check_bundle import validate
  target=pathlib.Path(json.loads(subprocess.check_output(['cargo','metadata','--offline','--locked','--no-deps','--format-version','1'],cwd=ROOT))['target_directory'])/'release/voiceport-rs'
  swift=pathlib.Path(subprocess.check_output(['swift','build','-c','release','--show-bin-path'],cwd=ROOT,text=True).strip())/'VoicePort'
  for kind in ['main','unknown-both','unknown-declaration','unknown-selection','omitted-package']:
   with tempfile.TemporaryDirectory() as t:
    a=pathlib.Path(t)/'VoicePort.app';shutil.copytree(ROOT/'dist/VoicePort.app',a);p=a/'Contents/Resources/third-party/manifest.json';m=json.loads(p.read_text())
    if kind=='main':shutil.copyfile(swift.parent/'voiceport-cli',a/'Contents/MacOS/VoicePort')
    else:
     if kind in ['unknown-both','unknown-declaration']:m['packages'][0]['declared_spdx']='UNKNOWN'
     if kind in ['unknown-both','unknown-selection']:m['packages'][0]['selection']='UNKNOWN'
     if kind=='omitted-package':
      for f in m['packages'].pop(0)['materials']:(p.parent/f['path']).unlink()
     p.write_text(json.dumps(m))
    subprocess.run(['codesign','--force','--sign','-',a],check=True,capture_output=True);subprocess.run(['codesign','--verify','--deep','--strict',a],check=True,capture_output=True)
    self.assertEqual((a/'Contents/Helpers/voiceport-rs').read_bytes(),(ROOT/'dist/VoicePort.app/Contents/Helpers/voiceport-rs').read_bytes())
    with self.assertRaisesRegex(AssertionError,'main does not match' if kind=='main' else 'installed license manifest'):validate(a,target,swift)
 def test_actual_entry_rejects_initial_link_and_observes_runtime_drift(self):
  inputs=inventory(ROOT)
  for case in ['initial-link','runtime-drift']:
   with tempfile.TemporaryDirectory() as t:
    scope=pathlib.Path(t).resolve();tree=scope/'product';tree.mkdir()
    for d in inputs['directories']:(tree/d).mkdir(parents=True,exist_ok=True);(tree/d).chmod(inputs['directory_modes'][d])
    for f in inputs['files']:
     p=tree/f['path'];p.write_bytes((ROOT/f['path']).read_bytes());p.chmod(f['mode'])
    environment=dict(os.environ,PYTHONDONTWRITEBYTECODE='1',CARGO_NET_OFFLINE='true',CODEX_HOME=str(scope/'absent-codex'),TMPDIR=str(scope))
    if case=='initial-link':
     (tree/'docs').rename(scope/'held-docs');(tree/'docs').symlink_to(scope/'held-docs',target_is_directory=True)
    else:
     tools=scope/'tools';tools.mkdir();cargo=tools/'cargo';cargo.write_text('#!/bin/sh\nif [ "$1" = test ]; then echo changed >> "'+str(tree/'README.md')+'"; echo "0 failed"; exit 0; fi\nexit 7\n');cargo.chmod(0o755);environment['PATH']=str(tools)+os.pathsep+environment['PATH']
    argv=['python3','-I',str(tree/'scripts/verify.py'),'--app-smoke'];result=subprocess.run(argv,cwd=scope,env=environment,capture_output=True,timeout=30)
    report=json.loads(next((tree/'.voiceport/evidence/vp012-revision-01').glob('verify-*/report.json')).read_text())
    print(json.dumps({'case':case,'argv':argv,'cwd':str(scope),'PATH_seam':environment['PATH'] if case=='runtime-drift' else None,'exit':result.returncode,'report':report}),file=sys.stderr);sys.stderr.write(result.stdout.decode()+result.stderr.decode())
    self.assertEqual(result.returncode,1);self.assertFalse(report['passed'])
    if case=='initial-link':self.assertEqual(report['checks'],[])
    else:self.assertFalse(report['inputs_unchanged']);self.assertEqual([r['exit_code'] for r in report['checks']],[0,7])
