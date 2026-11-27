# Matched Legacy and Records head-to-head over one pinned installation (BE-28, build engine architecture section 13).
#
#   run.rb seal-prefill BAKERY RECEIPT
#   run.rb build-runner OUTPUT --source CHECKOUT --prefill BAKERY
#   run.rb compare [--untimed] --output DIR --source CHECKOUT --archive TOOLCHAIN_ARCHIVE --pin PIN_JSON
#          --prefill BAKERY --prefill-receipt RECEIPT --cache-note TEXT [--arms hello,two-homes,graph-lsp,census]
#          [--contracts-runner RUNNER --oracle VERDICTS] [--slots PAR_SLOT_DIRECTORY] [--pairs N] [--census-pairs N]
#          [--census-jobs 5,12] [--census-states cold,warm] [--retire-trees]
#   run.rb summarize OUTPUT
#
# Run under par.rb: the installed home is INCAN_HOME, and INCAN must be its launcher.
require 'optparse'
require_relative 'census'
require_relative 'graph_lsp'
require_relative 'pins'
require_relative 'summary'
require_relative 'two_homes'

module HeadToHead
  ARMS = %w[hello two-homes graph-lsp census].freeze

  def self.installed_home
    home = File.realpath(ENV.fetch('INCAN_HOME'))
    refuse('INCAN is not the home launcher') unless File.realpath(ENV.fetch('INCAN')) == File.realpath(File.join(home, 'bin/incan'))
    refuse('timing must run untraced') unless TRACE_FLAGS.none? { |flag| ENV.key?(flag) }
    home
  end

  def self.options(arguments)
    settings = { timed: true, arms: ARMS, census_jobs: [5, 12], census_states: %w[cold warm] }
    OptionParser.new do |parser|
      parser.on('--untimed') { settings[:timed] = false }
      parser.on('--retire-trees') { settings[:retire_trees] = true }
      %i[output source archive pin prefill prefill_receipt cache_note contracts_runner oracle slots].each do |name|
        parser.on("--#{name.to_s.tr('_', '-')} VALUE") { |value| settings[name] = value }
      end
      parser.on('--arms LIST', Array) { |value| settings[:arms] = value }
      parser.on('--pairs N', Integer) { |value| settings[:pairs] = value }
      parser.on('--census-pairs N', Integer) { |value| settings[:census_pairs] = value }
      parser.on('--census-jobs LIST', Array) { |value| settings[:census_jobs] = value.map { |jobs| Integer(jobs) } }
      parser.on('--census-states LIST', Array) { |value| settings[:census_states] = value }
    end.parse!(arguments)
    settings[:pairs] ||= settings[:timed] ? 20 : 1
    settings[:census_pairs] ||= settings[:timed] ? 5 : 1
    settings[:slot_root] = settings.delete(:slots)
    settings
  end

  def self.validate(settings)
    %i[output source archive pin prefill prefill_receipt cache_note].each do |key|
      refuse("missing --#{key.to_s.tr('_', '-')}") if settings[key].to_s.strip.empty?
    end
    unknown = settings.fetch(:arms) - ARMS
    refuse("unknown arms #{unknown.join(', ')}") unless unknown.empty?
    refuse('census states are cold and warm, cold first') unless [%w[cold], %w[cold warm]].include?(settings.fetch(:census_states))
    if (settings.fetch(:arms) & %w[census]).any?
      refuse('the census needs --contracts-runner and --oracle') unless settings[:contracts_runner] && settings[:oracle]
    end
    %i[source archive prefill prefill_receipt contracts_runner oracle].each do |key|
      settings[key] = File.realpath(settings.fetch(key)) if settings[key]
    end
    output = File.join(File.realpath(File.dirname(settings.fetch(:output))), File.basename(settings.fetch(:output)))
    refuse("output already exists: #{output}") if File.exist?(output)
    settings.merge(output: output, home: installed_home)
  end

  # Inputs frozen before the first sample; the oracle and stock runner are pinned when the census runs.
  def self.inputs(settings, expected)
    frozen = Pins.freeze(settings, expected)
    if settings[:oracle]
      refuse('the census oracle differs from the pin') unless sha256(settings.fetch(:oracle)) == expected.fetch('oracle_sha256')
      frozen[:census] = { oracle: settings.fetch(:oracle), oracle_sha256: sha256(settings.fetch(:oracle)),
                          runner: settings[:contracts_runner], runner_sha256: settings[:contracts_runner] && sha256(settings[:contracts_runner]),
                          pass_baseline_sha256: sha256(File.join(settings.fetch(:source), 'workspaces/contracts/pass-baseline.txt')) }
    end
    environment = ENV.to_h.slice('INCAN_RUST_INCREMENTAL', 'RUSTUP_TOOLCHAIN', 'OVEN_OWNER', 'INCAN_NO_BANNER', 'PATH', 'TMPDIR')
    frozen.merge(settings: settings.reject { |key, _| key == :pin }.merge(pin_file: settings.fetch(:pin)),
                 started_utc: utc, trace: 'off', inherited_environment: environment,
                 profiles: { hello: 'debug', two_homes: 'debug', graph_lsp: 'release', census: 'stock runner cases' })
  end

  def self.run_arm(session, arm, expected)
    pairs = session.settings.fetch(:pairs)
    case arm
    when 'hello' then (1..pairs).each { |pair| Hello.run_pair(session, pair) }
    when 'two-homes'
      homes = TwoHomes.install(session)
      (1..pairs).each { |pair| TwoHomes.run_pair(session, pair, homes) }
      TwoHomes.retire_homes(session, homes)
    when 'graph-lsp'
      arms = GraphLsp.setup(session)
      references = {}
      (1..pairs).each { |pair| GraphLsp.run_pair(session, pair, arms, references) }
    when 'census'
      session.settings.fetch(:census_jobs).each do |jobs|
        (1..session.settings.fetch(:census_pairs)).each { |pair| Census.run_pair(session, pair, jobs, expected) }
      end
    end
    true
  rescue Refusal => refusal
    session.fail_arm(arm, refusal)
    warn "#{arm}: #{refusal.message}"
    false
  ensure
    # Leave no worker of this home or of the run's secondary homes behind for the next arm.
    quiesce([session.home, session.output])
  end

  # Prime the home's catalog once per engine, then pin it: samples must leave it unchanged.
  def self.setup(session)
    primed = Hello.prime(session, session.home, 'primary')
    catalog = File.join(session.home, 'bakery')
    setup = { primed_exits: primed, home_catalog: catalog, home_catalog_sha256: Pins.inventory_digest(catalog), utc: utc }
    record(File.join(session.output, 'setup.json'), setup)
    setup
  end

  def self.compare(arguments)
    settings = validate(options(arguments))
    expected = read_json(settings.fetch(:pin))
    frozen = inputs(settings, expected)
    make_directory(settings.fetch(:output))
    record(File.join(settings.fetch(:output), 'inputs.json'), frozen)
    census = settings[:oracle] && { oracle: Census.verdicts(settings.fetch(:oracle)), floor: Census.pass_floor(settings.fetch(:source)) }
    session = Session.new(settings)
    prepared = setup(session)
    results = settings.fetch(:arms).to_h { |arm| [arm, run_arm(session, arm, census)] }
    %w[setup graph-lsp].each { |part| session.retire(File.join(settings.fetch(:output), part)) if results.fetch(part, true) }
    session.close
    catalog = Pins.inventory_digest(prepared.fetch(:home_catalog))
    record(File.join(settings.fetch(:output), 'result.json'),
           { ended_utc: utc, arms: results, home_catalog_sha256: catalog,
             home_catalog_unchanged: catalog == prepared.fetch(:home_catalog_sha256) })
    Summary.write(settings.fetch(:output))
    puts JSON.generate(results)
    exit(results.values.all? ? 0 : 1)
  end

  # Build the stock Contracts runner as the installed verifier does, on a clone of the prefill.
  def self.build_runner(arguments)
    output = arguments.shift
    settings = { source: nil, prefill: nil }
    OptionParser.new do |parser|
      parser.on('--source VALUE') { |value| settings[:source] = File.realpath(value) }
      parser.on('--prefill VALUE') { |value| settings[:prefill] = File.realpath(value) }
    end.parse!(arguments)
    refuse("output already exists: #{output}") if File.exist?(output)
    home = installed_home
    project = make_directory(File.join(output, 'runner'))
    FileUtils.cp(File.join(settings.fetch(:source), 'workspaces/contracts/loaf.toml'), project)
    FileUtils.cp_r(File.join(settings.fetch(:source), 'workspaces/contracts/src'), project)
    bakery = clone_tree(settings.fetch(:prefill), File.join(output, 'bakery'))
    build = observe(project, { 'INCAN_BAKERY' => bakery, 'INCAN_ENGINE' => nil },
                    [File.join(home, 'bin/oven'), 'build', 'src/main.incn'], timed: false, slot_root: nil)
    File.binwrite(File.join(output, 'runner-build.log'), build.fetch(:stdout) + build.fetch(:stderr))
    runner = File.join(project, 'target/incan/debug/bin/contracts')
    built = build.fetch(:exit).zero? && File.executable?(runner)
    record(File.join(output, 'runner.json'),
           { built_utc: utc, home: home, source: Pins.git(settings.fetch(:source), 'rev-parse', 'HEAD'),
             contracts_tree: Pins.git(settings.fetch(:source), 'rev-parse', 'HEAD:workspaces/contracts/src'),
             argv: build.fetch(:argv), exit: build.fetch(:exit), runner: built ? runner : nil,
             runner_sha256: built ? sha256(runner) : nil })
    refuse('the stock Contracts runner did not build') unless built
    puts runner
  end

  def self.main(arguments)
    command = arguments.shift
    case command
    when 'seal-prefill'
      refuse('usage: run.rb seal-prefill BAKERY RECEIPT') unless arguments.size == 2
      Pins.seal_prefill(arguments[0], arguments[1], installed_home)
    when 'build-runner' then build_runner(arguments)
    when 'compare' then compare(arguments)
    when 'summarize'
      refuse('usage: run.rb summarize OUTPUT') unless arguments.size == 1
      Summary.write(File.realpath(arguments[0]))
    else refuse('usage: run.rb seal-prefill|build-runner|compare|summarize ...')
    end
  rescue Refusal => refusal
    warn refusal.message
    warn JSON.generate(refusal.evidence) if refusal.evidence
    exit 1
  end
end

HeadToHead.main(ARGV.dup)
