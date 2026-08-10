<?php
/**
 * Construction-level compatibility for credential-gated Google libraries that
 * have no emulator: google/ads-googleads (the issue-#13 version_compare
 * class) and google/shopping-merchant-products (issue #18's library).
 *
 * Instantiating the generated gRPC service clients exercises extension
 * detection, phpversion('grpc') gates, gax transport construction, and class
 * inheritance against our classes — everything short of an authenticated RPC.
 */
declare(strict_types=1);

require '/l1/vendor/autoload.php';

$tests = 0;
$passed = 0;
function check(string $name, bool $r, string $detail = ''): void {
    global $tests, $passed;
    $tests++;
    if ($r) { $passed++; echo "  ✓ {$name}\n"; }
    else { echo "  ✗ {$name}" . ($detail ? ": {$detail}" : '') . "\n"; }
}

echo "=== google/ads-googleads + shopping-merchant-products (construction) ===\n\n";

check('ext-grpc reported version acceptable to google-ads',
    version_compare(phpversion('grpc'), '1.57.0', '>='), phpversion('grpc'));

// Fake service-account keyfile: valid shape, un-usable key — construction
// must succeed; only a real RPC would fail on it.
$keyfile = [
    'type' => 'service_account',
    'project_id' => 'test-project',
    'private_key_id' => 'fake',
    'private_key' => "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQg" .
        str_repeat('A', 43) . "\n-----END PRIVATE KEY-----\n",
    'client_email' => 'fake@test-project.iam.gserviceaccount.com',
    'client_id' => '0',
];

// google/ads-googleads: find the newest bundled version namespace
$adsClient = null;
$adsVersion = '?';
foreach (range(40, 15) as $v) {
    $cls = "Google\\Ads\\GoogleAds\\V{$v}\\Services\\Client\\GoogleAdsServiceClient";
    if (class_exists($cls)) {
        $adsVersion = "V{$v}";
        try {
            $adsClient = new $cls(['credentials' => $keyfile]);
        } catch (Throwable $e) {
            check("ads {$adsVersion} client constructs", false, get_class($e) . ': ' . $e->getMessage());
        }
        break;
    }
}
check("ads client class found ({$adsVersion})", $adsVersion !== '?');
if ($adsClient !== null) {
    check("ads {$adsVersion} GoogleAdsServiceClient constructs over grpc transport", true);
}

// google/shopping-merchant-products
try {
    $cls = 'Google\Shopping\Merchant\Products\V1beta\Client\ProductInputsServiceClient';
    if (!class_exists($cls)) {
        $cls = 'Google\Shopping\Merchant\Products\V1\Client\ProductInputsServiceClient';
    }
    $merchant = new $cls(['credentials' => $keyfile, 'transport' => 'grpc']);
    check('merchant ProductInputsServiceClient constructs over grpc transport', true);
} catch (Throwable $e) {
    check('merchant ProductInputsServiceClient constructs over grpc transport', false,
        get_class($e) . ': ' . $e->getMessage());
}

echo "\n=== {$passed}/{$tests} tests passed ===\n";
exit($passed === $tests ? 0 : 1);
