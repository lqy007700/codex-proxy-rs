require 'digest'
require 'fileutils'
require 'json'
require_relative 'release-metadata'

# 归档与镜像消费同一次构建的二进制与 Web 资源
module ReleasePackage
  APP_NAME = 'codex-proxy-rs'

  def self.legacy_update_manifest
    version = ReleaseMetadata.version(ENV.fetch('VERSION'))
    git_sha = ENV.fetch('GIT_SHA')
    raise 'Invalid release Git SHA' unless git_sha.match?(/\A[0-9a-f]{40}\z/)

    # 旧更新器在替换程序前读取此路径；空合同保留插件风险确认，不声明已移除的能力
    JSON.pretty_generate(
      'schema_version' => 1,
      'sealed' => true,
      'gateway_version' => version,
      'gateway_git_sha' => git_sha,
      'plugin_host' => nil,
      'plugins' => []
    ) + "\n"
  end

  def self.archives
    platforms = JSON.parse(ENV.fetch('PLATFORM_MATRIX')).fetch('include')
    manifest = legacy_update_manifest
    release_dir = 'dist/release'
    FileUtils.rm_rf(release_dir)
    FileUtils.mkdir_p(release_dir)
    FileUtils.chmod_R('u+w', 'package') if File.directory?('package')
    FileUtils.rm_rf('package')
    config = 'deploy/config.example.yaml'
    FileUtils.cp(config, release_dir)
    compose = File.read('deploy/compose.yaml')
    image = "#{ENV.fetch('REGISTRY')}/#{ENV.fetch('IMAGE_NAME')}"
    default_image = "${CPR_IMAGE:-#{image}:latest}"
    raise 'Expected exactly one default application image in deploy/compose.yaml' unless compose.scan(default_image).length == 1
    File.write("#{release_dir}/compose.yaml", compose.sub(default_image, "${CPR_IMAGE:-#{image}:#{ENV.fetch('DOCKER_VERSION')}}"))

    platforms.each do |platform|
      raise "Unsupported archive format: #{platform['archive']}" unless platform['archive'] == 'tar.gz'
      name = "#{platform.fetch('os')}-#{platform.fetch('arch')}"
      binary = platform.fetch('binary')
      root = "package/#{name}/#{APP_NAME}"
      FileUtils.mkdir_p(["#{root}/web/dist", "#{root}/deploy"])
      FileUtils.cp("dist/binaries/#{APP_NAME}-#{name}/#{binary}", "#{root}/#{binary}")
      File.chmod(0o755, "#{root}/#{binary}")
      FileUtils.cp_r('dist/frontend/.', "#{root}/web/dist")
      FileUtils.cp(config, "#{root}/deploy")
      legacy_directory = "#{root}/plugins/official"
      FileUtils.mkdir_p(legacy_directory)
      legacy_manifest = "#{legacy_directory}/plugin-release-manifest.json"
      File.write(legacy_manifest, manifest)
      File.chmod(0o444, legacy_manifest)
      archive = "#{release_dir}/#{APP_NAME}_#{ENV.fetch('VERSION')}_#{platform['os']}_#{platform['arch']}.tar.gz"
      raise "Failed to create #{archive}" unless system('tar', '-czf', archive, '-C', File.dirname(root), APP_NAME)
    end

    checksums = Dir.glob("#{release_dir}/*").sort.map { |path| "#{Digest::SHA256.file(path).hexdigest}  #{File.basename(path)}\n" }
    File.write("#{release_dir}/checksums.txt", checksums.join)
  end

  def self.image
    platform = "#{ENV.fetch('MATRIX_OS')}-#{ENV.fetch('MATRIX_ARCH')}"
    target = "dist/docker/#{platform}"
    FileUtils.mkdir_p(target)
    FileUtils.cp("dist/docker-artifacts/#{APP_NAME}-#{platform}/#{ENV.fetch('MATRIX_BINARY')}", "#{target}/#{APP_NAME}")
    File.chmod(0o755, "#{target}/#{APP_NAME}")
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    case ARGV
    when ['archives'] then ReleasePackage.archives
    when ['image'] then ReleasePackage.image
    else raise 'usage: package-release.rb archives|image'
    end
  rescue StandardError => error
    warn "error: #{error.message}"
    exit 1
  end
end
