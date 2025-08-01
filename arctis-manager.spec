# -*- mode: python ; coding: utf-8 -*-
from pathlib import Path
import sys
import subprocess

from PyInstaller.utils.hooks import collect_submodules

sys.path.append('.')

hiddenimports = ['PyQt6', 'PyQt6.sip']
hiddenimports += collect_submodules('arctis_manager.devices')

python_ver_p = subprocess.run('python --version', shell=True, check=True, stdout=subprocess.PIPE)
python_ver = '.'.join(python_ver_p.stdout.decode('utf-8').replace('Python ', '').split('.')[0:2])
site_packages_p = subprocess.run('python3 -c "import sysconfig; print(sysconfig.get_path(\'purelib\'))"', shell=True, check=True, stdout=subprocess.PIPE)
pyqt6_path = Path(site_packages_p.stdout.decode('utf-8')).parent.parent.parent.joinpath('lib64', f'python{python_ver}', 'site-packages', 'PyQt6', 'Qt6')

print(str(pyqt6_path))

a = Analysis(
    ['arctis_manager.py'],
    pathex=['.'],
    binaries=[],
    datas=[
        ('arctis_manager/images/steelseries_logo.svg', 'arctis_manager/images/'),
        ('arctis_manager/lang/*.json', 'arctis_manager/lang/'),
        (pyqt6_path.joinpath('plugins', 'platforms'), 'PyQt6/Qt6/plugins/platforms/'),
    ],
    hiddenimports=hiddenimports,
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    [],
    name='arctis-manager',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=True,
    upx_exclude=[],
    runtime_tmpdir=None,
    console=True,
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
)
