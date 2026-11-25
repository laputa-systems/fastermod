#!/usr/bin/env python3
import argparse
import datetime
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKLOADS = {
    'search_matching_1mb_file': 'search',
    'search_missing_1mb_file': 'search',
    'replace_all_rust_10k': 'replace',
    'diff_sparse_rust_10k': 'diff',
    'patch_single_1mb_file': 'patch',
    'run_fast_no_match_100_files': 'tree',
    'run_fast_match_100_files': 'tree',
    'cli_text_apply': 'cli',
    'cli_binding_rename': 'cli',
    'cli_saved_plan': 'cli',
    'cli_interactive_review': 'cli',
}
TOOL_FLOORS = {'python3': (3, 9), 'cargo': (1, 85), 'rustc': (1, 85)}
GATE_TOOLS = ('cpu-guard', 'perf-gate-ready')
MIN_FREE_BYTES = 1024 ** 3
BENCH_FIXTURE_MARKER = 'fastermod-bench-'


def select(names, areas, skips):
    unknown = (set(names) | set(skips)) - WORKLOADS.keys()
    unknown_areas = set(areas) - set(WORKLOADS.values())
    if unknown or unknown_areas:
        raise ValueError(f'unknown names or areas: {sorted(unknown | unknown_areas)}')
    selected = [n for n, area in WORKLOADS.items()
                if (not names or n in names) and (not areas or area in areas) and n not in skips]
    if not selected:
        raise ValueError('selection matches no workloads')
    return selected


def parse_version(text, tool):
    match = re.search(r'(\d+)\.(\d+)', text)
    return (int(match.group(1)), int(match.group(2))) if match else None


def version_problem(tool, version):
    floor = TOOL_FLOORS.get(tool)
    if version is None:
        return f'{tool} did not report a parseable version'
    if floor and version < floor:
        return f'{tool} {version[0]}.{version[1]} below required {floor[0]}.{floor[1]}'
    return None


def tool_versions(tools):
    versions = {}
    for tool in tools:
        found = shutil.which(tool)
        if not found:
            versions[tool] = {'path': None, 'version': None, 'problem': f'{tool} missing'}
            continue
        args = ['--version'] if tool in TOOL_FLOORS else ['--help']
        result = subprocess.run([found, *args], capture_output=True, text=True)
        text = result.stdout + result.stderr
        version = parse_version(text, tool) if tool in TOOL_FLOORS else None
        problem = version_problem(tool, version) if tool in TOOL_FLOORS else None
        if tool not in TOOL_FLOORS and result.returncode != 0:
            problem = f'{tool} help unavailable'
        versions[tool] = {
            'path': found,
            'version': f'{version[0]}.{version[1]}' if version else None,
            'problem': problem,
        }
    return versions


def gate_status():
    executable = shutil.which('perf-gate-ready')
    if not executable:
        return {'code': None, 'detail': 'perf-gate-ready missing', 'quiet': False}
    result = subprocess.run([executable, '-1'], capture_output=True, text=True)
    detail = (result.stdout + result.stderr).strip()
    return {'code': result.returncode, 'detail': detail, 'quiet': result.returncode == 0}


def timing_validity(gate):
    if gate is None or not gate.get('quiet'):
        return False, 'host gate not quiet'
    return True, ''


def valid_budget(sample_count, sample_size, repeat):
    return sample_count > 0 and sample_size > 0 and repeat > 0


def stale_inputs():
    binaries = sorted(
        (p for p in (ROOT / 'target').glob('**/deps/bench-*') if p.is_file() and p.suffix != '.d'),
        key=lambda p: p.stat().st_mtime,
        reverse=True,
    )
    if not binaries:
        return []
    built = binaries[0].stat().st_mtime
    sources = [ROOT / 'Cargo.toml', ROOT / 'Cargo.lock']
    sources += list((ROOT / 'src').glob('**/*.rs'))
    sources += list((ROOT / 'benches').glob('**/*.rs'))
    return [str(p.relative_to(ROOT)) for p in sources if p.is_file() and p.stat().st_mtime > built]


def process_table():
    proc = pathlib.Path('/proc')
    if proc.is_dir():
        rows = []
        for entry in proc.glob('[0-9]*'):
            try:
                args = (entry / 'cmdline').read_bytes().replace(b'\0', b' ').decode()
            except OSError:
                continue
            rows.append((int(entry.name), args))
        return rows
    result = subprocess.run(['ps', '-eo', 'pid=,args='], capture_output=True, text=True)
    if result.returncode:
        return []
    rows = []
    for line in result.stdout.splitlines():
        parts = line.strip().split(maxsplit=1)
        if len(parts) == 2:
            rows.append((int(parts[0]), parts[1]))
    return rows


def leftover_processes():
    current = {os.getpid(), os.getppid()}
    return [
        {'pid': pid, 'args': args}
        for pid, args in process_table()
        if pid not in current
        and (BENCH_FIXTURE_MARKER in args or args.startswith('cpu-guard --cores'))
    ]


def preflight(args, selected):
    problems = []
    tools = {}
    budget = {
        'sample_count': args.sample_count,
        'sample_size': args.sample_size,
        'repeat': args.repeat,
    }
    if args.tier == 'edit':
        tools['python3'] = {
            'path': sys.executable,
            'version': f'{sys.version_info.major}.{sys.version_info.minor}',
            'problem': version_problem('python3', sys.version_info[:2]),
        }
        if not (ROOT / 'tools/bench_tier.py').is_file():
            problems.append('selection self-test source missing')
    else:
        if not valid_budget(args.sample_count, args.sample_size, args.repeat):
            problems.append('sample count, sample size and repeat must all be positive')
        tools = tool_versions(('cargo', 'rustc', *GATE_TOOLS))
        problems += [v['problem'] for v in tools.values() if v['problem']]
        if shutil.disk_usage(ROOT).free < MIN_FREE_BYTES:
            problems.append('less than 1 GiB free')
    stale = stale_inputs()
    running = leftover_processes() if args.tier != 'edit' else []
    problems += [f'stale input: {name}' for name in stale]
    if running:
        problems.append('benchmark or guard processes already running')
    gate = None if args.tier == 'edit' else gate_status()
    report = {
        'tier': args.tier,
        'selected': selected,
        'tools': tools,
        'host_gate': gate,
        'budget': budget,
        'stale_inputs': stale,
        'leftover_processes': running,
        'problems': problems,
    }
    return report


def run_directory():
    output = ROOT / '.work' / 'bench' / datetime.datetime.now().strftime('%Y%m%d-%H%M%S-%f')
    output.mkdir(parents=True)
    return output


def write_report(directory, action, report):
    report['action'] = action
    report['verdict'] = 'none; completion and validity evidence only'
    (directory / 'report.json').write_text(json.dumps(report, indent=2, sort_keys=True))


def excluded(selected, problems, gate):
    _, reason = timing_validity(gate)
    reason = '; '.join([reason, *problems]).strip('; ')
    return {name: reason or 'not excluded' for name in selected}


def print_summary(report, validity, output):
    for name in report['selected']:
        valid, reason = validity[name]
        state = 'timing valid' if valid else f'excluded: {reason}'
        print(f'{name}: {state}')
    print(f"report={output / 'report.json'}; completion is not a performance verdict", flush=True)


def bench_command(root, selected, args):
    return [
        'cpu-guard', '--cores', '4', 'cargo', 'bench', '--locked', '--bench', 'bench',
        '--', '--exact', '--sample-count', str(args.sample_count),
        '--sample-size', str(args.sample_size), *(f'bench::{name}' for name in selected),
    ]


def environment():
    env = os.environ.copy()
    env['CARGO_BUILD_JOBS'] = '4'
    env.setdefault('TMPDIR', str(ROOT / '.work/tmp'))
    return env


def run(args, selected, report, directory):
    if args.tier == 'edit':
        command = [sys.executable, str(ROOT / 'tools/bench_tier.py'), 'selftest']
    else:
        command = bench_command(ROOT, selected, args)
    print('command:', ' '.join(command), flush=True)
    if args.dry_run:
        return
    start = time.monotonic()
    result = subprocess.run(command, cwd=ROOT, env=environment(),
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    (directory / 'output.log').write_text(result.stdout)
    reasons = excluded(selected, report['problems'], report['host_gate'])
    validity = {
        name: (result.returncode == 0 and name in result.stdout and reasons[name] == 'not excluded',
               reasons[name] if reasons[name] != 'not excluded' else
               ('workload missing from output' if name not in result.stdout else
                ('command failed' if result.returncode else '')))
        for name in selected
    }
    report['workloads'] = {
        name: {'timing_valid': valid, 'reason': reason, 'exit': result.returncode}
        for name, (valid, reason) in validity.items()
    }
    print(result.stdout, end='')
    print(f'exit={result.returncode} duration={time.monotonic() - start:.2f}s')
    print_summary(report, validity, directory)
    write_report(directory, 'run', report)
    if result.returncode:
        sys.exit(result.returncode)


def compare(args, selected, report, directory):
    baseline = pathlib.Path(args.baseline_root).resolve()
    candidate = pathlib.Path(args.candidate_root).resolve()
    if not baseline.is_dir() or not candidate.is_dir():
        raise ValueError('compare requires two existing checkout roots')
    if report['problems'] or not report['host_gate']['quiet']:
        reasons = excluded(selected, report['problems'], report['host_gate'])
        validity = {name: (False, reasons[name]) for name in selected}
        report['workloads'] = {
            name: {'timing_valid': False, 'reason': reason, 'legs': []}
            for name, (_, reason) in validity.items()
        }
        print_summary(report, validity, directory)
        write_report(directory, 'compare', report)
        return

    legs = []
    validity = {}
    for index, name in enumerate(selected):
        order = [('baseline', baseline), ('candidate', candidate)] * args.repeat
        if index % 2:
            order.reverse()
        per_workload = []
        for side, root in order:
            before = gate_status()
            command = bench_command(root, [name], args)
            start = time.monotonic()
            result = subprocess.run(command, cwd=root, env=environment(),
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            after = gate_status()
            log = directory / f'{name}-{side}-{len(per_workload)}.log'
            log.write_text(result.stdout)
            leg = {
                'workload': name, 'side': side, 'exit': result.returncode,
                'gate_before': before['code'], 'gate_after': after['code'],
                'duration_s': round(time.monotonic() - start, 2), 'log': log.name,
            }
            per_workload.append(leg)
            legs.append(leg)
        quiet = all(leg['exit'] == 0 and leg['gate_before'] == 0 and leg['gate_after'] == 0
                    for leg in per_workload)
        reason = '' if quiet else 'a paired leg failed or overlapped host load'
        validity[name] = (quiet, reason)
    report['workloads'] = {
        name: {'timing_valid': valid, 'reason': reason,
               'legs': [leg for leg in legs if leg['workload'] == name]}
        for name, (valid, reason) in validity.items()
    }
    report['paired_budget_equal'] = True
    print_json = json.dumps(report['workloads'], indent=2, sort_keys=True)
    print(print_json)
    print_summary(report, validity, directory)
    write_report(directory, 'compare', report)


def selftest():
    assert select(['search_missing_1mb_file'], [], []) == ['search_missing_1mb_file']
    assert len(select([], ['search'], [])) == 2
    assert len(select([], ['cli'], [])) == 4
    for a in [(['bad'], [], []), ([], ['bad'], []), ([], [], ['bad']),
              (['search_missing_1mb_file'], [], ['search_missing_1mb_file'])]:
        try:
            select(*a)
        except ValueError:
            continue
        raise AssertionError(a)
    assert parse_version('Python 3.15.1', 'python3') == (3, 15)
    assert parse_version('cargo 1.102.0-nightly (c07ac2c4d 2026-11-06)', 'cargo') == (1, 102)
    assert version_problem('cargo', (1, 84)) is not None
    assert version_problem('cargo', (1, 85)) is None
    assert timing_validity({'quiet': True}) == (True, '')
    assert timing_validity({'quiet': False})[0] is False
    assert valid_budget(1, 1, 1)
    assert not valid_budget(0, 1, 1)
    assert not valid_budget(1, 0, 1)
    assert not valid_budget(1, 1, 0)
    print('selection, version, budget and validity checks passed')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('action', choices=['check', 'list', 'run', 'compare', 'selftest'])
    parser.add_argument('--tier', choices=['edit', 'change'], default='edit')
    parser.add_argument('--name', action='append', default=[])
    parser.add_argument('--area', action='append', default=[])
    parser.add_argument('--skip', action='append', default=[])
    parser.add_argument('--sample-count', type=int, default=1)
    parser.add_argument('--sample-size', type=int, default=1)
    parser.add_argument('--repeat', type=int, default=1)
    parser.add_argument('--baseline-root')
    parser.add_argument('--candidate-root')
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    if args.action == 'selftest':
        selftest()
        return
    if args.action == 'list':
        for name, area in WORKLOADS.items():
            print(f'change {area} {name}')
        print('edit harness selection selftest')
        return
    try:
        selected = select(args.name, args.area, args.skip) if args.tier == 'change' else []
        if args.tier == 'edit' and (args.name or args.area or args.skip):
            raise ValueError('edit tier has no workload selectors')
        if args.action == 'compare' and args.tier != 'change':
            raise ValueError('compare requires the change tier')
        report = preflight(args, selected)
        if report['problems']:
            raise ValueError('; '.join(report['problems']))
    except ValueError as error:
        parser.error(str(error))
    for name in WORKLOADS:
        if name not in selected:
            print(f'excluded {name}: not selected')
    if report['host_gate'] is not None:
        print(f"host gate: {'quiet' if report['host_gate']['quiet'] else 'busy'}; "
              'a quiet gate is required for paired timing')
    if args.action == 'check':
        print('preflight passed')
        return
    directory = run_directory()
    if args.action == 'compare':
        compare(args, selected, report, directory)
    else:
        run(args, selected, report, directory)


if __name__ == '__main__':
    main()
