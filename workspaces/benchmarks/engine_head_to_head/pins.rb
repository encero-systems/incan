# Freeze the experiment before measuring: installation, host, source, fixtures, oracle and prefill identities.
require_relative 'support'

module HeadToHead
  module Pins
    RUNNER_DIRECTORY = 'workspaces/benchmarks/engine_head_to_head'.freeze

    def self.git(source, *arguments)
      output, errors, status = Open3.capture3('/usr/bin/git', '-C', source, *arguments)
      HeadToHead.refuse("git #{arguments.join(' ')} failed: #{errors}") unless status.success?
      output.strip
    end

    def self.command(*argv)
      output, status = Open3.capture2(*argv)
      HeadToHead.refuse("#{argv.join(' ')} failed") unless status.success?
      output.strip
    end

    # The installed launcher, LSP, selected manifest, driver, compiler records and SDK of one home.
    def self.installation(home)
      manifest = File.join(home, 'toolchains/0.6.0-dev.9/toolchain.json')
      selected = HeadToHead.read_json(manifest)
      driver = File.expand_path(selected.fetch('driver'), File.dirname(manifest))
      sdk = File.join(home, 'cache/apple-sdk.json')
      { home: home, launcher_sha256: HeadToHead.sha256(File.join(home, 'bin/incan')),
        lsp_sha256: HeadToHead.sha256(File.join(home, 'bin/incan-lsp')),
        oven_alias: File.readlink(File.join(home, 'bin/oven')), manifest: manifest,
        manifest_sha256: HeadToHead.sha256(manifest), driver: driver, driver_sha256: HeadToHead.sha256(driver),
        compiler_records: selected.fetch('compiler_records').fetch('digest'),
        sdk_root: File.file?(sdk) ? HeadToHead.read_json(sdk).fetch('root') : nil }
    end

    def self.host
      { model: command('/usr/sbin/sysctl', '-n', 'hw.model'),
        cpu: command('/usr/sbin/sysctl', '-n', 'machdep.cpu.brand_string'),
        logical_cpus: Integer(command('/usr/sbin/sysctl', '-n', 'hw.ncpu')),
        memory_bytes: Integer(command('/usr/sbin/sysctl', '-n', 'hw.memsize')),
        os: command('/usr/bin/sw_vers', '-productVersion'), os_build: command('/usr/bin/sw_vers', '-buildVersion'),
        kernel: command('/usr/bin/uname', '-v') }
    end

    # The checkout must be the pinned language-core revision plus, at most, changes to this runner.
    def self.source(source, expected)
      head = git(source, 'rev-parse', 'HEAD')
      base = expected.fetch('source')
      git(source, 'merge-base', '--is-ancestor', base, head)
      changed = git(source, 'diff', '--name-only', base, head).lines.map(&:strip)
      foreign = changed.reject { |path| path.start_with?(RUNNER_DIRECTORY + '/') }
      HeadToHead.require_fact(foreign.empty?, "checkout changes compared paths since #{base}", foreign)
      dirty = git(source, 'status', '--porcelain', '--', 'workspaces/contracts', RUNNER_DIRECTORY)
      HeadToHead.require_fact(dirty.empty?, 'checkout has uncommitted compared or runner paths', dirty)
      { head: head, language_core: base, runner_changes: changed,
        fixtures_tree: git(source, 'rev-parse', 'HEAD:workspaces/contracts/fixtures'),
        pass_baseline_blob: git(source, 'rev-parse', 'HEAD:workspaces/contracts/pass-baseline.txt'),
        runner_tree: changed.empty? ? nil : git(source, 'rev-parse', "HEAD:#{RUNNER_DIRECTORY}") }
    end

    # Every file of a quiescent prefilled Bakery, so each clone can be traced to the exact admitted bytes.
    def self.inventory(bakery)
      files = Dir.glob('**/*', File::FNM_DOTMATCH, base: bakery).sort.select do |relative|
        File.file?(File.join(bakery, relative)) && !File.symlink?(File.join(bakery, relative))
      end
      files.to_h do |relative|
        path = File.join(bakery, relative)
        [relative, { bytes: File.size(path), sha256: HeadToHead.sha256(path) }]
      end
    end

    def self.inventory_digest(directory)
      Digest::SHA256.hexdigest(inventory(directory).map { |relative, entry| "#{entry.fetch(:sha256)}  #{relative}\n" }.join)
    end

    def self.seal_prefill(bakery, receipt, home)
      bakery = File.realpath(bakery)
      HeadToHead.require_fact(!File.exist?(receipt), "prefill receipt already exists: #{receipt}")
      workers = HeadToHead.home_workers([home])
      HeadToHead.require_fact(workers.empty?, 'the home has live workers; the prefill is not quiescent', workers)
      open_files, = Open3.capture2('/usr/sbin/lsof', '+D', bakery)
      files = inventory(bakery)
      HeadToHead.require_fact(Dir.empty?(File.join(bakery, 'receipts')), 'the prefill already holds project receipts')
      listing = files.map { |relative, entry| "#{entry.fetch(:sha256)}  #{relative}\n" }.join
      HeadToHead.record(receipt, { schema: 'incan.head-to-head.prefill/1', sealed_utc: HeadToHead.utc, bakery: bakery,
                                   state: 'prefilled cold: admitted bundle roots, no project results',
                                   source_home: home, open_files: open_files.lines.drop(1),
                                   files: files.size, bytes: files.values.sum { |entry| entry.fetch(:bytes) },
                                   inventory_sha256: Digest::SHA256.hexdigest(listing), inventory: files })
    end

    # Refuse a run whose installation, oracle or prefill differ from the frozen identities.
    def self.freeze(settings, expected)
      installation = installation(settings.fetch(:home))
      %w[launcher_sha256 lsp_sha256 manifest_sha256].each do |key|
        HeadToHead.require_fact(installation.fetch(key.to_sym) == expected.fetch(key), "installed #{key} differs from the pin")
      end
      HeadToHead.require_fact(HeadToHead.sha256(settings.fetch(:archive)) == expected.fetch('archive_sha256'),
                              'toolchain archive differs from the pin')
      source = source(settings.fetch(:source), expected)
      %w[fixtures_tree pass_baseline_blob].each do |key|
        HeadToHead.require_fact(source.fetch(key.to_sym) == expected.fetch(key), "checkout #{key} differs from the pin")
      end
      prefill = HeadToHead.read_json(settings.fetch(:prefill_receipt))
      HeadToHead.require_fact(inventory(settings.fetch(:prefill)).transform_values { |entry| entry.transform_keys(&:to_s) } ==
                              prefill.fetch('inventory'), 'the prefilled Bakery differs from its sealed receipt')
      { pin: expected, installation: installation, host: host, source: source,
        prefill: { bakery: settings.fetch(:prefill), receipt: settings.fetch(:prefill_receipt),
                   receipt_sha256: HeadToHead.sha256(settings.fetch(:prefill_receipt)),
                   inventory_sha256: prefill.fetch('inventory_sha256') } }
    end
  end
end
